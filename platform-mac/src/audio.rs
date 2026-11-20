use std::{
    ffi::{CStr, c_void},
    mem::MaybeUninit,
    ptr::{self, NonNull},
    slice,
};

use core_engine::{
    Result,
    audio::{AudioMeter, BAND_COUNT},
};
use core_foundation::{
    array::CFArray,
    base::{CFType, TCFType},
    boolean::CFBoolean,
    dictionary::CFDictionary,
    string::CFString,
};
use libloading::os::unix::Library;
use objc2::{
    msg_send, msg_send_id,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Bool},
};
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart,
    AudioDeviceStop, AudioHardwareCreateAggregateDevice, AudioHardwareDestroyAggregateDevice,
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey,
    kAudioAggregateDeviceUIDKey, kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal,
    kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey, kAudioTapPropertyFormat,
};
use objc2_core_audio_types::{
    AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp, kAudioFormatFlagIsBigEndian,
    kAudioFormatFlagIsFloat, kAudioFormatLinearPCM,
};
use objc2_foundation::{NSArray, NSNumber, NSUUID};
use rtrb::{Consumer, Producer, RingBuffer};

const BUFFER_SAMPLES: usize = 8192;
type CreateTap = unsafe extern "C" fn(*const AnyObject, *mut AudioObjectID) -> i32;
type DestroyTap = unsafe extern "C" fn(AudioObjectID) -> i32;

pub struct Capture {
    device: AggregateDevice,
    _tap: ProcessTap,
    io_proc: AudioDeviceIOProcID,
    producer: NonNull<Producer<f32>>,
    consumer: Consumer<f32>,
    scratch: Box<[f32; BUFFER_SAMPLES]>,
    meter: AudioMeter,
    sample_rate: u32,
    running: bool,
}

impl Capture {
    pub fn new() -> Result<Self> {
        let (tap, uid) = ProcessTap::new()?;
        let sample_rate = tap.sample_rate()?;
        let device = AggregateDevice::new(&uid)?;
        let (producer, consumer) = RingBuffer::new(BUFFER_SAMPLES);
        let producer = NonNull::from(Box::leak(Box::new(producer)));
        let mut io_proc = None;
        // HAL keeps the fixed callback context pointer until the IOProc is destroyed.
        let status = unsafe {
            AudioDeviceCreateIOProcID(
                device.0,
                Some(read_playback),
                producer.cast().as_ptr(),
                NonNull::from(&mut io_proc),
            )
        };
        if let Err(error) = check(status, "register the playback callback") {
            // Registration failed, so HAL does not own or use the callback context.
            unsafe { drop(Box::from_raw(producer.as_ptr())) };
            return Err(error);
        }
        if io_proc.is_none() {
            return Err("Core Audio returned no playback callback handle".into());
        }
        let meter = AudioMeter::default();
        meter.set_active(false);
        Ok(Self {
            device,
            _tap: tap,
            io_proc,
            producer,
            consumer,
            scratch: Box::new([0.0; BUFFER_SAMPLES]),
            meter,
            sample_rate,
            running: false,
        })
    }

    pub fn set_running(&mut self, running: bool) -> Result<()> {
        if running == self.running {
            return Ok(());
        }
        if running {
            let _ = self.consumer.pop_partial_slice(self.scratch.as_mut_slice());
            // The private aggregate and its callback remain registered for this Capture.
            check(
                unsafe { AudioDeviceStart(self.device.0, self.io_proc) },
                "start system audio",
            )?;
        } else {
            check(
                unsafe { AudioDeviceStop(self.device.0, self.io_proc) },
                "pause system audio",
            )?;
        }
        self.running = running;
        self.meter.set_active(running);
        Ok(())
    }

    pub fn bands(&mut self) -> [f32; BAND_COUNT] {
        let (samples, _) = self.consumer.pop_partial_slice(self.scratch.as_mut_slice());
        self.meter.push_mono(samples, self.sample_rate);
        self.meter.bands()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.set_running(false);
        // On teardown failure leave the context in place in case HAL is still using it.
        unsafe {
            if AudioDeviceDestroyIOProcID(self.device.0, self.io_proc) == 0 {
                drop(Box::from_raw(self.producer.as_ptr()));
            }
        }
    }
}

struct ProcessTap {
    id: AudioObjectID,
    destroy: DestroyTap,
}

impl ProcessTap {
    fn new() -> Result<(Self, String)> {
        let class =
            AnyClass::get("CATapDescription").ok_or("System audio requires macOS 14.2 or newer")?;
        let library = Library::this();
        // These APIs are optional on older macOS versions. The image exporting
        // them is CoreAudio, which is already linked for the stable HAL calls.
        let (create, destroy) = unsafe {
            (
                *library.get::<CreateTap>(b"AudioHardwareCreateProcessTap\0")?,
                *library.get::<DestroyTap>(b"AudioHardwareDestroyProcessTap\0")?,
            )
        };
        // An empty exclusion list taps all application playback. The default
        // CATapDescription mute behavior keeps normal playback audible.
        let excluded = NSArray::<NSNumber>::new();
        let description: Retained<AnyObject> = unsafe {
            let allocated: *mut AnyObject = msg_send![class, alloc];
            let initialized: *mut AnyObject =
                msg_send![allocated, initMonoGlobalTapButExcludeProcesses: &*excluded];
            Retained::from_raw(initialized)
        }
        .ok_or("Could not describe the global playback tap")?;
        unsafe {
            let _: () = msg_send![&*description, setPrivate: Bool::YES];
        }
        let uuid: Option<Retained<NSUUID>> = unsafe { msg_send_id![&*description, UUID] };
        let uid = uuid
            .ok_or("Core Audio returned no tap UUID")?
            .UUIDString()
            .to_string();
        let mut id = 0;
        // The description is a retained CATapDescription and id is writable.
        check(
            unsafe { create(Retained::as_ptr(&description), &mut id) },
            "create the playback tap",
        )?;
        Ok((Self { id, destroy }, uid))
    }

    fn sample_rate(&self) -> Result<u32> {
        let mut address = AudioObjectPropertyAddress {
            mSelector: kAudioTapPropertyFormat,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        };
        let expected_size = size_of::<AudioStreamBasicDescription>() as u32;
        let mut size = expected_size;
        let mut format = MaybeUninit::<AudioStreamBasicDescription>::uninit();
        // The stream format property writes one AudioStreamBasicDescription.
        check(
            unsafe {
                AudioObjectGetPropertyData(
                    self.id,
                    NonNull::from(&mut address),
                    0,
                    ptr::null(),
                    NonNull::from(&mut size),
                    NonNull::from(&mut format).cast(),
                )
            },
            "read the playback format",
        )?;
        if size != expected_size {
            return Err("Core Audio returned an incomplete playback format".into());
        }
        let format = unsafe { format.assume_init() };
        if format.mFormatID != kAudioFormatLinearPCM
            || format.mFormatFlags & kAudioFormatFlagIsFloat == 0
            || format.mFormatFlags & kAudioFormatFlagIsBigEndian != 0
            || format.mBitsPerChannel != 32
            || format.mBytesPerFrame != 4
            || format.mChannelsPerFrame != 1
            || !(1.0..=f64::from(u32::MAX)).contains(&format.mSampleRate)
        {
            return Err(
                "Core Audio returned an unsupported format for the mono playback tap".into(),
            );
        }
        Ok(format.mSampleRate.round() as u32)
    }
}

impl Drop for ProcessTap {
    fn drop(&mut self) {
        unsafe { (self.destroy)(self.id) };
    }
}

struct AggregateDevice(AudioObjectID);

impl AggregateDevice {
    fn new(uid: &str) -> Result<Self> {
        let yes = CFBoolean::true_value().as_CFType();
        let tap = dictionary(&[
            (kAudioSubTapUIDKey, CFString::new(uid).as_CFType()),
            (kAudioSubTapDriftCompensationKey, yes.clone()),
        ]);
        // The aggregate only has this playback tap. It does not open a microphone
        // or change the default device used by other applications.
        let description = dictionary(&[
            (
                kAudioAggregateDeviceUIDKey,
                CFString::new(&format!("{uid}-wallp")).as_CFType(),
            ),
            (
                kAudioAggregateDeviceNameKey,
                CFString::from_static_string("Desktop Wallpapers Playback").as_CFType(),
            ),
            (kAudioAggregateDeviceIsPrivateKey, yes.clone()),
            (kAudioAggregateDeviceTapAutoStartKey, yes),
            (
                kAudioAggregateDeviceTapListKey,
                CFArray::from_CFTypes(&[tap]).as_CFType(),
            ),
        ]);
        let definition = description
            .as_concrete_TypeRef()
            .cast::<objc2_core_foundation::CFDictionary>();
        let mut id = 0;
        // Both bindings refer to the same Core Foundation dictionary, which is
        // retained by description for the duration of the call.
        check(
            unsafe { AudioHardwareCreateAggregateDevice(&*definition, NonNull::from(&mut id)) },
            "open the private playback device",
        )?;
        Ok(Self(id))
    }
}

impl Drop for AggregateDevice {
    fn drop(&mut self) {
        unsafe { AudioHardwareDestroyAggregateDevice(self.0) };
    }
}

fn dictionary(values: &[(&CStr, CFType)]) -> CFDictionary<CFString, CFType> {
    let values: Vec<_> = values
        .iter()
        .map(|(key, value)| (CFString::new(&key.to_string_lossy()), value.clone()))
        .collect();
    CFDictionary::from_CFType_pairs(&values)
}

fn check(status: i32, operation: &str) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("Could not {operation} (Core Audio status {status})").into())
    }
}

unsafe extern "C-unwind" fn read_playback(
    _: AudioObjectID,
    _: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _: NonNull<AudioTimeStamp>,
    _: NonNull<AudioBufferList>,
    _: NonNull<AudioTimeStamp>,
    context: *mut c_void,
) -> i32 {
    // HAL owns the input buffer for this callback and calls this IOProc serially.
    // Read the list length before borrowing a buffer, since HAL can return zero.
    if unsafe { ptr::addr_of!((*input.as_ptr()).mNumberBuffers).read() } != 1 {
        return 0;
    }
    let buffer = unsafe { &(*input.as_ptr()).mBuffers[0] };
    let bytes = buffer.mDataByteSize as usize;
    let data = buffer.mData.cast::<f32>();
    if context.is_null()
        || buffer.mNumberChannels != 1
        || data.is_null()
        || !data.is_aligned()
        || !bytes.is_multiple_of(size_of::<f32>())
    {
        return 0;
    }
    let samples = unsafe { slice::from_raw_parts(data, bytes / size_of::<f32>()) };
    let producer = unsafe { &mut *context.cast::<Producer<f32>>() };
    let _ = producer.push_partial_slice(samples);
    0
}
