//! A small, filtered texture keeps icon work out of the full-resolution shader.

pub const FIELD_WIDTH: u32 = 320;
pub const FIELD_HEIGHT: u32 = 180;
const PIXELS: usize = (FIELD_WIDTH * FIELD_HEIGHT) as usize;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Icon {
    pub center: [f32; 2],
    pub kind: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IconLayout {
    /// The stamp radii in normalized output coordinates.
    pub radius: [f32; 2],
    pub icons: Vec<Icon>,
}

impl IconLayout {
    pub(crate) fn demo() -> Self {
        Self {
            radius: [0.074, 0.13],
            icons: [
                [0.2, 0.31],
                [0.27, 0.42],
                [0.33, 0.33],
                [0.29, 0.54],
                [0.71, 0.57],
                [0.78, 0.64],
                [0.72, 0.72],
            ]
            .into_iter()
            .enumerate()
            .map(|(index, center)| Icon {
                center,
                kind: (index % 5) as u8,
            })
            .collect(),
        }
    }

    /// Red is combined influence, green is the dominant ingredient,
    /// blue is the strongest single stamp, and alpha is overlap.
    pub(crate) fn rasterize(&self) -> Vec<u8> {
        let mut combined = vec![0.0_f32; PIXELS];
        let mut strongest = vec![0.0_f32; PIXELS];
        let mut kind = vec![0_u8; PIXELS];
        let [rx, ry] = [
            self.radius[0].clamp(0.005, 0.5) * FIELD_WIDTH as f32,
            self.radius[1].clamp(0.005, 0.5) * FIELD_HEIGHT as f32,
        ];
        for icon in &self.icons {
            let [x, y] = [
                icon.center[0] * FIELD_WIDTH as f32,
                icon.center[1] * FIELD_HEIGHT as f32,
            ];
            let left = ((x - rx).floor() as i32).max(0);
            let right = ((x + rx).ceil() as i32).min(FIELD_WIDTH as i32);
            let top = ((y - ry).floor() as i32).max(0);
            let bottom = ((y + ry).ceil() as i32).min(FIELD_HEIGHT as i32);
            for row in top..bottom {
                for col in left..right {
                    let dx = (col as f32 + 0.5 - x) / rx;
                    let dy = (row as f32 + 0.5 - y) / ry;
                    let strength = (1.0 - dx * dx - dy * dy).max(0.0).powi(2);
                    let i = (row * FIELD_WIDTH as i32 + col) as usize;
                    combined[i] += strength;
                    if strength > strongest[i] {
                        strongest[i] = strength;
                        kind[i] = (icon.kind % 5) * 51 + 25;
                    }
                }
            }
        }
        let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        let mut rgba = Vec::with_capacity(PIXELS * 4);
        for i in 0..PIXELS {
            rgba.extend([
                byte(combined[i]),
                kind[i],
                byte(strongest[i]),
                byte(combined[i] - strongest[i]),
            ]);
        }
        rgba
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighboring_icons_merge_and_the_old_site_clears_after_a_move() {
        let mut layout = IconLayout {
            radius: [0.12, 0.17],
            icons: vec![
                Icon {
                    center: [0.20, 0.5],
                    kind: 0,
                },
                Icon {
                    center: [0.34, 0.5],
                    kind: 4,
                },
            ],
        };
        let middle = ((FIELD_HEIGHT / 2 * FIELD_WIDTH + 86) * 4) as usize;
        let before = layout.rasterize();
        assert!(before[middle] > 175);
        assert!(before[middle + 3] > 50);
        for icon in &mut layout.icons {
            icon.center[0] += 0.5;
        }
        let after = layout.rasterize();
        let moved = middle + (FIELD_WIDTH / 2 * 4) as usize;
        assert_eq!(after[middle], 0);
        assert_eq!(after[moved], before[middle]);
    }
}
