use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

const INTERVAL: Duration = Duration::from_secs(2);

pub struct BatterySaver {
    next_poll: Option<Instant>,
    active: bool,
}

impl BatterySaver {
    pub fn new(enabled: bool) -> Self {
        Self {
            next_poll: enabled.then(Instant::now),
            active: false,
        }
    }

    pub fn update(&mut self, now: Instant) -> bool {
        if self.next_poll.is_some_and(|deadline| now >= deadline) {
            self.active = on_battery(Path::new("/sys/class/power_supply"));
            self.next_poll = Some(now + INTERVAL);
        }
        self.active
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.next_poll
    }
}

fn on_battery(root: &Path) -> bool {
    let Ok(supplies) = fs::read_dir(root) else {
        return false;
    };
    let mut discharging = false;
    for supply in supplies.flatten() {
        let path = supply.path();
        let matches = |property, value| {
            fs::read_to_string(path.join(property)).is_ok_and(|text| text.trim() == value)
        };
        if matches("scope", "Device") {
            continue;
        }
        if matches("type", "Battery") {
            discharging |= matches("status", "Discharging");
        } else if matches("online", "1") {
            return false;
        }
    }
    discharging
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_discharging_system_battery_with_no_online_adapter_pauses() {
        let root = std::env::temp_dir().join(format!("wallp-linux-power-{}", std::process::id()));
        let battery = root.join("BAT0");
        let adapter = root.join("AC");
        fs::create_dir_all(&battery).unwrap();
        fs::create_dir_all(&adapter).unwrap();
        fs::write(battery.join("type"), "Battery\n").unwrap();
        fs::write(battery.join("status"), "Discharging\n").unwrap();
        fs::write(adapter.join("type"), "Mains\n").unwrap();
        fs::write(adapter.join("online"), "0\n").unwrap();
        let on_dc = on_battery(&root);
        fs::write(adapter.join("online"), "1\n").unwrap();
        let on_ac = on_battery(&root);
        fs::write(adapter.join("online"), "0\n").unwrap();
        fs::write(battery.join("scope"), "Device\n").unwrap();
        let on_peripheral = on_battery(&root);
        fs::remove_dir_all(root).unwrap();
        assert!(on_dc);
        assert!(!on_ac);
        assert!(!on_peripheral);
    }
}
