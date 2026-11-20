//! A small, filtered texture keeps icon work out of the full-resolution shader.

pub const FIELD_WIDTH: u32 = 320;
pub const FIELD_HEIGHT: u32 = 180;
const PIXELS: usize = (FIELD_WIDTH * FIELD_HEIGHT) as usize;
pub(crate) const FIELD_BYTES: usize = PIXELS * 4;

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
        let mut rgba = Vec::with_capacity(FIELD_BYTES);
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

    /// Red is the link core, green is the position along that link, blue is its halo.
    pub(crate) fn rasterize_links(&self) -> Vec<u8> {
        let mut edges = Vec::new();
        let radius = self.radius.map(|value| value.clamp(0.005, 0.5));
        // ponytail: search the bounded icon list only when the layout changes.
        // One candidate per direction limits the amount of ink in dense clusters.
        for (i, icon) in self.icons.iter().enumerate() {
            let mut nearest: [Option<(usize, f32)>; 4] = [None; 4];
            for (j, other) in self.icons.iter().enumerate() {
                let dx = (other.center[0] - icon.center[0]) / radius[0];
                let dy = (other.center[1] - icon.center[1]) / radius[1];
                let distance = dx * dx + dy * dy;
                if !(0.04..=2.1).contains(&distance) {
                    continue;
                }
                let direction = if dx.abs() >= dy.abs() {
                    usize::from(dx > 0.0)
                } else {
                    2 + usize::from(dy > 0.0)
                };
                if nearest[direction].is_none_or(|(_, current)| distance < current) {
                    nearest[direction] = Some((j, distance));
                }
            }
            edges.extend(
                nearest
                    .into_iter()
                    .flatten()
                    .map(|(j, _)| (i.min(j), i.max(j))),
            );
        }
        edges.sort_unstable();
        edges.dedup();

        let mut rgba = vec![0; FIELD_BYTES];
        let center = |i: usize| {
            let [x, y] = self.icons[i].center;
            [x * FIELD_WIDTH as f32, y * FIELD_HEIGHT as f32]
        };
        let width = ((radius[0] * FIELD_WIDTH as f32 + radius[1] * FIELD_HEIGHT as f32) * 0.05)
            .clamp(1.4, 3.0);
        let halo = width * 3.0;
        for (i, j) in edges {
            let [ax, ay] = center(i);
            let [bx, by] = center(j);
            let length = (bx - ax).hypot(by - ay);
            let [dx, dy] = [(bx - ax) / length, (by - ay) / length];
            let left = ((ax.min(bx) - halo).floor() as i32).max(0);
            let right = ((ax.max(bx) + halo).ceil() as i32).min(FIELD_WIDTH as i32);
            let top = ((ay.min(by) - halo).floor() as i32).max(0);
            let bottom = ((ay.max(by) + halo).ceil() as i32).min(FIELD_HEIGHT as i32);
            for row in top..bottom {
                for col in left..right {
                    let x = col as f32 + 0.5 - ax;
                    let y = row as f32 + 0.5 - ay;
                    let along = (x * dx + y * dy).clamp(0.0, length);
                    let distance = (x - along * dx).hypot(y - along * dy);
                    let core = byte(1.0 - distance / width);
                    let glow = byte((1.0 - distance / halo).max(0.0).powi(2));
                    let offset = (row * FIELD_WIDTH as i32 + col) as usize * 4;
                    if glow > rgba[offset + 2] {
                        rgba[offset] = core;
                        rgba[offset + 1] = byte(along / length);
                        rgba[offset + 2] = glow;
                    }
                }
            }
        }
        rgba
    }
}

fn byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
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

    #[test]
    fn links_connect_neighbors_and_carry_travel_progress() {
        let mut layout = IconLayout {
            radius: [0.1, 0.15],
            icons: vec![
                Icon {
                    center: [0.25, 0.5],
                    kind: 0,
                },
                Icon {
                    center: [0.35, 0.5],
                    kind: 1,
                },
                Icon {
                    center: [0.8, 0.5],
                    kind: 2,
                },
            ],
        };
        let sample = |column| ((FIELD_HEIGHT / 2 * FIELD_WIDTH + column) * 4) as usize;
        let first = layout.rasterize_links();
        assert!(first[sample(96)] > 150);
        assert!(first[sample(88) + 1] < first[sample(103) + 1]);
        assert_eq!(first[sample(185) + 2], 0);
        layout.icons[1].center[0] = 0.57;
        assert_eq!(layout.rasterize_links()[sample(96)], 0);
    }
}
