//! Fly camera in the world's Z-up frame.

use glam::{Mat4, Vec3};

pub struct Camera {
    pub position: Vec3,
    /// Radians, 0 = looking along +Y (north), increases turning left.
    pub yaw: f32,
    /// Radians, positive = looking up.
    pub pitch: f32,
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
    pub speed: f32,
}

impl Camera {
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(-sy * cp, cy * cp, sp)
    }

    pub fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::Z).normalize_or(Vec3::X)
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_to_rh(self.position, self.forward(), Vec3::Z)
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, aspect, self.near, self.far) * self.view()
    }

    pub fn look(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.003;
        self.pitch = (self.pitch - dy * 0.003).clamp(-1.5, 1.5);
    }
}
