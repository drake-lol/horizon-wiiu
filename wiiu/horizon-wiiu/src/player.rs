//! The player: walking physics with an axis-aligned box, and the block ray cast. Units are blocks and seconds;
//! a frame is 1/60 s.

use crate::world::World;

const HALF_WIDTH: f32 = 0.3;
const HEIGHT: f32 = 1.8;
pub const EYE: f32 = 1.62;
const DT: f32 = 1.0 / 60.0;
const GRAVITY: f32 = 32.0;
const WALK: f32 = 4.3;
const JUMP: f32 = 9.0;
pub const REACH: f32 = 5.0;

#[derive(Default, Clone, Copy)]
pub struct Input {
    /// Strafe right (+) and forward (+), each -1..1.
    pub strafe: f32,
    pub forward: f32,
    /// Turn left (+) and look up (+), each -1..1.
    pub look_x: f32,
    pub look_y: f32,
    pub jump: bool,
}

pub struct Player {
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

impl Player {
    pub fn new(pos: [f32; 3]) -> Player {
        Player { pos, vel: [0.0; 3], yaw: 0.0, pitch: 0.0, on_ground: false }
    }

    pub fn eye(&self) -> [f32; 3] {
        [self.pos[0], self.pos[1] + EYE, self.pos[2]]
    }

    pub fn forward_vec(&self) -> [f32; 3] {
        [-self.yaw.sin() * self.pitch.cos(), self.pitch.sin(), -self.yaw.cos() * self.pitch.cos()]
    }

    fn collides(world: &World, p: [f32; 3]) -> bool {
        let (x0, x1) = ((p[0] - HALF_WIDTH).floor() as i32, (p[0] + HALF_WIDTH).floor() as i32);
        let (y0, y1) = (p[1].floor() as i32, (p[1] + HEIGHT).floor() as i32);
        let (z0, z1) = ((p[2] - HALF_WIDTH).floor() as i32, (p[2] + HALF_WIDTH).floor() as i32);
        for y in y0..=y1 {
            for z in z0..=z1 {
                for x in x0..=x1 {
                    if world.get(x, y, z).solid() {
                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn update(&mut self, world: &World, input: &Input) {
        self.yaw += input.look_x * 0.045;
        self.pitch = (self.pitch + input.look_y * 0.03).clamp(-1.5, 1.5);

        let (s, c) = (self.yaw.sin(), self.yaw.cos());
        let (fwd, right) = ([-s, -c], [c, -s]);
        let (mut mx, mut mz) = (right[0] * input.strafe + fwd[0] * input.forward, right[1] * input.strafe + fwd[1] * input.forward);
        let len = (mx * mx + mz * mz).sqrt();
        if len > 1.0 {
            mx /= len;
            mz /= len;
        }
        self.vel[0] = mx * WALK;
        self.vel[2] = mz * WALK;
        if input.jump && self.on_ground {
            self.vel[1] = JUMP;
        }
        self.vel[1] = (self.vel[1] - GRAVITY * DT).max(-60.0);

        // one axis at a time, so the box slides along walls
        for axis in [0usize, 2, 1] {
            let mut next = self.pos;
            next[axis] += self.vel[axis] * DT;
            if Self::collides(world, next) {
                if axis == 1 && self.vel[1] < 0.0 {
                    self.on_ground = true;
                }
                self.vel[axis] = 0.0;
            } else {
                if axis == 1 {
                    self.on_ground = false;
                }
                self.pos = next;
            }
        }
        // keep inside the loaded area vertically
        if self.pos[1] < -10.0 {
            self.pos[1] = world.sy as f32 + 2.0;
            self.vel[1] = 0.0;
        }
    }
}

/// The first solid block along the ray, its position and the face normal that was hit (Amanatides & Woo).
pub fn raycast(world: &World, origin: [f32; 3], dir: [f32; 3], reach: f32) -> Option<([i32; 3], [i32; 3])> {
    let mut cell = [origin[0].floor() as i32, origin[1].floor() as i32, origin[2].floor() as i32];
    let mut step = [0i32; 3];
    let mut t_max = [f32::INFINITY; 3];
    let mut t_delta = [f32::INFINITY; 3];
    for i in 0..3 {
        if dir[i] > 0.0 {
            step[i] = 1;
            t_delta[i] = 1.0 / dir[i];
            t_max[i] = (cell[i] as f32 + 1.0 - origin[i]) / dir[i];
        } else if dir[i] < 0.0 {
            step[i] = -1;
            t_delta[i] = -1.0 / dir[i];
            t_max[i] = (origin[i] - cell[i] as f32) / -dir[i];
        }
    }
    let mut normal = [0i32; 3];
    loop {
        if world.get(cell[0], cell[1], cell[2]).solid() {
            return Some((cell, normal));
        }
        let axis = if t_max[0] < t_max[1] && t_max[0] < t_max[2] {
            0
        } else if t_max[1] < t_max[2] {
            1
        } else {
            2
        };
        if t_max[axis] > reach || step[axis] == 0 {
            return None;
        }
        cell[axis] += step[axis];
        t_max[axis] += t_delta[axis];
        normal = [0; 3];
        normal[axis] = -step[axis];
    }
}
