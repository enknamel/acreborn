//! A rendered map as pixels plus the transform between world metres and
//! pixels: the world map of Dereth, a landblock's local map or a
//! dungeon's floor plan all come out as one of these, and the map panel
//! draws them the same way.

use glam::Vec2;

/// RGBA8 pixels, row 0 at the top (north).
#[derive(Clone, Debug, PartialEq)]
pub struct MapImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// World x and y of the image's south-west corner (pixel column 0,
    /// bottom row).
    pub origin: Vec2,
    /// Pixels per metre.
    pub scale: f32,
}

impl MapImage {
    /// A blank (transparent) image covering `size` metres from `origin`.
    pub fn blank(origin: Vec2, size: Vec2, scale: f32) -> MapImage {
        let width = (size.x * scale).ceil().max(1.0) as u32;
        let height = (size.y * scale).ceil().max(1.0) as u32;
        MapImage {
            width,
            height,
            rgba: vec![0; width as usize * height as usize * 4],
            origin,
            scale,
        }
    }

    /// World extent in metres: `origin` to `origin + size()`.
    pub fn size(&self) -> Vec2 {
        Vec2::new(self.width as f32, self.height as f32) / self.scale
    }

    /// Pixel position (x right, y down, fractional) of a world xy.
    pub fn to_pixel(&self, world: Vec2) -> Vec2 {
        let d = (world - self.origin) * self.scale;
        Vec2::new(d.x, self.height as f32 - d.y)
    }

    /// World xy under a pixel position.
    pub fn to_world(&self, pixel: Vec2) -> Vec2 {
        let d = Vec2::new(pixel.x, self.height as f32 - pixel.y) / self.scale;
        self.origin + d
    }

    /// Whether a world xy is inside the image.
    pub fn contains(&self, world: Vec2) -> bool {
        let p = self.to_pixel(world);
        p.x >= 0.0 && p.y >= 0.0 && p.x < self.width as f32 && p.y < self.height as f32
    }

    /// Set one pixel; out-of-range coordinates are ignored.
    pub fn put(&mut self, x: i64, y: i64, rgba: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.rgba[i..i + 4].copy_from_slice(&rgba);
    }

    pub fn get(&self, x: i64, y: i64) -> Option<[u8; 4]> {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        Some([
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ])
    }

    /// Fill the axis-aligned box between two world corners.
    pub fn fill_world_rect(&mut self, a: Vec2, b: Vec2, rgba: [u8; 4]) {
        let p = self.to_pixel(a);
        let q = self.to_pixel(b);
        let (x0, x1) = (p.x.min(q.x).floor() as i64, p.x.max(q.x).ceil() as i64);
        let (y0, y1) = (p.y.min(q.y).floor() as i64, p.y.max(q.y).ceil() as i64);
        for y in y0..y1 {
            for x in x0..x1 {
                self.put(x, y, rgba);
            }
        }
    }

    /// Fill a world-space triangle (any winding).
    pub fn fill_world_tri(&mut self, a: Vec2, b: Vec2, c: Vec2, rgba: [u8; 4]) {
        let (a, b, c) = (self.to_pixel(a), self.to_pixel(b), self.to_pixel(c));
        let x0 = a.x.min(b.x).min(c.x).floor() as i64;
        let x1 = a.x.max(b.x).max(c.x).ceil() as i64;
        let y0 = a.y.min(b.y).min(c.y).floor() as i64;
        let y1 = a.y.max(b.y).max(c.y).ceil() as i64;
        let edge =
            |p: Vec2, q: Vec2, r: Vec2| (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
        let area = edge(a, b, c);
        if area.abs() < 1e-9 {
            return;
        }
        for y in y0..=y1 {
            for x in x0..=x1 {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let w0 = edge(b, c, p) / area;
                let w1 = edge(c, a, p) / area;
                let w2 = edge(a, b, p) / area;
                if w0 >= -1e-4 && w1 >= -1e-4 && w2 >= -1e-4 {
                    self.put(x, y, rgba);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_and_world_round_trip() {
        let m = MapImage::blank(Vec2::new(192.0, 384.0), Vec2::new(192.0, 192.0), 0.5);
        assert_eq!((m.width, m.height), (96, 96));
        // South-west corner is the bottom-left pixel.
        assert_eq!(m.to_pixel(Vec2::new(192.0, 384.0)), Vec2::new(0.0, 96.0));
        // North-east corner is the top-right.
        assert_eq!(m.to_pixel(Vec2::new(384.0, 576.0)), Vec2::new(96.0, 0.0));
        let w = Vec2::new(250.0, 400.0);
        let back = m.to_world(m.to_pixel(w));
        assert!((back - w).length() < 1e-3);
        assert!(m.contains(w));
        assert!(!m.contains(Vec2::new(100.0, 400.0)));
        assert_eq!(m.size(), Vec2::new(192.0, 192.0));
    }

    #[test]
    fn fills_rects_and_triangles() {
        let mut m = MapImage::blank(Vec2::ZERO, Vec2::new(10.0, 10.0), 1.0);
        m.fill_world_rect(Vec2::new(2.0, 2.0), Vec2::new(4.0, 4.0), [255, 0, 0, 255]);
        // World (2..4, 2..4) is pixel columns 2..4, rows 6..8 (row 0 is north).
        assert_eq!(m.get(2, 6), Some([255, 0, 0, 255]));
        assert_eq!(m.get(3, 7), Some([255, 0, 0, 255]));
        assert_eq!(m.get(2, 5), Some([0, 0, 0, 0]));
        assert_eq!(m.get(4, 6), Some([0, 0, 0, 0]));
        m.fill_world_tri(
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(0.0, 10.0),
            [0, 255, 0, 255],
        );
        // Below the diagonal is green, above it stays clear.
        assert_eq!(m.get(1, 9), Some([0, 255, 0, 255]));
        assert_eq!(m.get(9, 1), Some([0, 0, 0, 0]));
        assert_eq!(m.get(20, 20), None);
        m.put(-1, 0, [1, 1, 1, 1]);
    }
}
