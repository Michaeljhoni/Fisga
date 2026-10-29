//! The holidays. In their weeks the main window takes the holiday's theme
//! (see `theme`): a scene drawn with cairo behind the panels and the cards,
//! moving for as long as the window is on screen (paused while it is
//! minimized): confetti in Carnaval, eggs and a bunny at Easter, a neon
//! heart for Dia dos Namorados, bunting at festa junina, bats in the fog at
//! Halloween, snow and fairy lights at Christmas, fireworks at New Year.
//! "Mostrar" in the preferences shows any of them while they are open.

use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{cairo, gdk, glib};

use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Season {
    Carnival,
    Easter,
    Valentines,
    June,
    Halloween,
    Christmas,
    NewYear,
}

/// A holiday's colors over the dark theme.
pub struct Look {
    pub accent: &'static str,
    /// The accent as text.
    pub accent_text: &'static str,
    /// Text on the accent.
    pub on_accent: &'static str,
    /// The speeds (the graph).
    pub graph: &'static str,
}

impl Season {
    /// In the order of the year.
    pub const ALL: [Season; 7] = [
        Season::Carnival,
        Season::Easter,
        Season::Valentines,
        Season::June,
        Season::Halloween,
        Season::Christmas,
        Season::NewYear,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Season::Carnival => "Carnaval",
            Season::Easter => "Páscoa",
            Season::Valentines => "Dia dos Namorados",
            Season::June => "Festa junina",
            Season::Halloween => "Halloween",
            Season::Christmas => "Natal",
            Season::NewYear => "Ano-novo",
        }
    }

    /// The name in `FISGA_SEASON`.
    fn name(self) -> &'static str {
        match self {
            Season::Carnival => "carnaval",
            Season::Easter => "pascoa",
            Season::Valentines => "namorados",
            Season::June => "junina",
            Season::Halloween => "halloween",
            Season::Christmas => "natal",
            Season::NewYear => "anonovo",
        }
    }

    pub fn look(self) -> Look {
        let (accent, accent_text, on_accent, graph) = match self {
            Season::Carnival => ("#d946ef", "#f0abfc", "#111111", "#f0abfc"),
            Season::Easter => ("#f9a8d4", "#fbcfe8", "#111111", "#f9a8d4"),
            Season::Valentines => ("#ff4d8d", "#ff9cc2", "#111111", "#ff9cc2"),
            Season::June => ("#f97316", "#fdba74", "#111111", "#fdba74"),
            Season::Halloween => ("#84cc16", "#bef264", "#111111", "#bef264"),
            Season::Christmas => ("#ef4444", "#fca5a5", "#ffffff", "#22d3ee"),
            Season::NewYear => ("#facc15", "#fde047", "#111111", "#22d3ee"),
        };
        Look { accent, accent_text, on_accent, graph }
    }

    /// The holiday whose week a date is in: Carnaval from the Friday to Ash
    /// Wednesday, Easter from Palm Sunday, 10–12 June (the Brazilian Dia dos
    /// Namorados), 20–29 June, 25–31 October, 18–26 December, and New Year
    /// from 27 December to 2 January.
    pub fn on(year: i32, month: u32, day: u32) -> Option<Season> {
        let date = day_number(year, month, day);
        let (easter_month, easter_day) = easter(year);
        let easter = day_number(year, easter_month, easter_day);
        if (easter - 51..=easter - 46).contains(&date) {
            return Some(Season::Carnival);
        }
        if (easter - 7..=easter).contains(&date) {
            return Some(Season::Easter);
        }
        match (month, day) {
            (6, 10..=12) => Some(Season::Valentines),
            (6, 20..=29) => Some(Season::June),
            (10, 25..=31) => Some(Season::Halloween),
            (12, 18..=26) => Some(Season::Christmas),
            (12, 27..=31) | (1, 1..=2) => Some(Season::NewYear),
            _ => None,
        }
    }

    /// Today's holiday; `FISGA_SEASON=natal` (or `nenhuma`) pretends, to
    /// see a theme out of its week.
    pub fn today() -> Option<Season> {
        if let Ok(name) = std::env::var("FISGA_SEASON") {
            return Season::ALL.into_iter().find(|season| season.name() == name);
        }
        let now = glib::DateTime::now_local().ok()?;
        Season::on(now.year(), now.month() as u32, now.day_of_month() as u32)
    }
}

/// Easter Sunday (month, day) of a Gregorian year (the anonymous algorithm).
fn easter(year: i32) -> (u32, u32) {
    let a = year % 19;
    let (b, c) = (year / 100, year % 100);
    let (d, e) = (b / 4, b % 4);
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let (i, k) = (c / 4, c % 4);
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    (month as u32, day as u32)
}

/// Days since 1970-01-01, to count days across months.
fn day_number(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = (i64::from(month) + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The scene's frame rate: 30 per second is plenty behind a download list.
const FRAME: i64 = 1_000_000 / 30;
/// What pulls the rockets down, in pixels/s².
const GRAVITY: f64 = 220.0;

const CONFETTI: [&str; 7] = ["#f472b6", "#facc15", "#34d399", "#60a5fa", "#a78bfa", "#fb923c", "#f87171"];
const PASTELS: [&str; 6] = ["#f9a8d4", "#a5f3fc", "#fde68a", "#c4b5fd", "#bbf7d0", "#fecaca"];
const HEARTS: [&str; 1] = ["#ff4d8d"];
const FLAGS: [&str; 7] = ["#ef4444", "#facc15", "#22c55e", "#3b82f6", "#f97316", "#a855f7", "#ec4899"];
const BULBS: [&str; 4] = ["#ef4444", "#facc15", "#22c55e", "#60a5fa"];
const FIREWORKS: [&str; 7] = ["#facc15", "#f87171", "#60a5fa", "#34d399", "#f472b6", "#c084fc", "#fb923c"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Confetti,
    Flake,
    Egg,
    Heart,
    Bat,
    Rocket,
    Spark,
}

struct Particle {
    shape: Shape,
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    /// Its turn (confetti), or a bat's heading.
    angle: f64,
    /// Its turning speed (confetti), or a bat's flying speed.
    spin: f64,
    size: f64,
    color: (f64, f64, f64),
    /// A second color: an egg's stripe.
    trim: (f64, f64, f64),
    /// Where its sway (or wing beat) starts.
    phase: f64,
    /// Seconds lived, and how many it may: sparks fade out, bats then fly away.
    age: f64,
    life: f64,
}

/// xorshift64*: enough for where the confetti falls.
struct Random(u64);

impl Random {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn between(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.next()
    }

    fn pick(&mut self, colors: &[&str]) -> (f64, f64, f64) {
        theme::rgb(colors[(self.next() * colors.len() as f64) as usize % colors.len()])
    }
}

/// One holiday's scene: its particles and the still parts placed at random.
struct Scene {
    season: Season,
    particles: Vec<Particle>,
    random: Random,
    /// Seconds of animation shown (not counting the time minimized).
    t: f64,
    /// Frame clock time (µs) of the last frame drawn; `None` after a pause.
    last: Option<i64>,
    /// The first particles are spread over the window, not all entering at once.
    filled: bool,
    /// Rockets owed by their rate.
    owed: f64,
    /// Stars: x and y (0–1 of the window), size, twinkle period and phase.
    stars: Vec<(f64, f64, f64, f64, f64)>,
    /// The skyline: width and height of each building, left to right.
    buildings: Vec<(f64, f64)>,
}

impl Scene {
    fn new(season: Season, seed: u64) -> Self {
        let mut random = Random(seed | 1);
        let count = match season {
            Season::June => 45,
            Season::NewYear => 24,
            _ => 0,
        };
        let stars = (0..count)
            .map(|_| (random.next(), random.next() * 0.7, random.between(0.6, 1.6), random.between(1.6, 4.2), random.between(0.0, 2.0 * PI)))
            .collect();
        let mut buildings = Vec::new();
        let mut width = 0.0;
        while width < 4000.0 {
            let building = (random.between(28.0, 72.0), random.between(26.0, 96.0));
            width += building.0 + 2.0;
            buildings.push(building);
        }
        Scene { season, particles: Vec::new(), random, t: 0.0, last: None, filled: false, owed: 0.0, stars, buildings }
    }

    /// How many particles keep the scene full, for a window of this size.
    fn population(&self, width: f64, height: f64) -> usize {
        let base = match self.season {
            Season::Carnival => 60.0,
            Season::Easter => 12.0,
            Season::Valentines => 8.0,
            Season::Halloween => 8.0,
            Season::Christmas => 90.0,
            Season::June | Season::NewYear => 0.0,
        };
        (base * (width * height / (960.0 * 528.0)).clamp(0.5, 2.5)).round() as usize
    }

    /// A new particle: entering the window, or anywhere in it (`inside`).
    fn spawn(&mut self, width: f64, height: f64, inside: bool) -> Particle {
        let r = &mut self.random;
        let mut p = Particle {
            shape: Shape::Confetti,
            x: r.between(0.0, width),
            y: if inside { r.between(0.0, height) } else { -20.0 },
            vx: 0.0,
            vy: 0.0,
            angle: 0.0,
            spin: 0.0,
            size: 0.0,
            color: (1.0, 1.0, 1.0),
            trim: (1.0, 1.0, 1.0),
            phase: r.between(0.0, 2.0 * PI),
            age: 0.0,
            life: f64::INFINITY,
        };
        match self.season {
            Season::Carnival => {
                p.vy = r.between(50.0, 90.0);
                p.angle = r.between(0.0, 2.0 * PI);
                p.spin = r.between(-5.0, 5.0);
                p.size = r.between(7.0, 12.0);
                p.color = r.pick(&CONFETTI);
            }
            Season::Easter => {
                p.shape = Shape::Egg;
                p.y = if inside { r.between(0.0, height) } else { -30.0 };
                p.vy = r.between(28.0, 48.0);
                p.size = r.between(14.0, 22.0);
                p.color = r.pick(&PASTELS);
                p.trim = r.pick(&PASTELS);
            }
            Season::Valentines => {
                p.shape = Shape::Heart;
                p.y = if inside { r.between(0.0, height) } else { height + 20.0 };
                p.vy = -r.between(25.0, 45.0);
                p.size = r.between(10.0, 16.0);
                p.color = r.pick(&HEARTS);
            }
            Season::Halloween => {
                // From a side, heading in; it wanders about, then flies away.
                p.shape = Shape::Bat;
                let from_left = r.next() < 0.5;
                if inside {
                    p.y = height * r.between(0.1, 0.7);
                    p.angle = r.between(0.0, 2.0 * PI);
                } else {
                    p.x = if from_left { -40.0 } else { width + 40.0 };
                    p.y = height * r.between(0.1, 0.6);
                    p.angle = if from_left { 0.0 } else { PI } + r.between(-0.4, 0.4);
                }
                p.spin = r.between(45.0, 85.0);
                p.size = r.between(24.0, 40.0);
                p.color = (0.016, 0.024, 0.02);
                p.life = r.between(18.0, 40.0);
            }
            Season::Christmas => {
                p.shape = Shape::Flake;
                p.y = if inside { r.between(0.0, height) } else { -6.0 };
                p.vy = r.between(25.0, 55.0);
                p.size = r.between(1.2, 3.0);
                p.color = if r.next() < 0.5 { (1.0, 1.0, 1.0) } else { theme::rgb("#e2ecf8") };
            }
            Season::NewYear => {
                // Climbs to a height between 12% and 50% of the window and bursts there.
                p.shape = Shape::Rocket;
                p.x = width * r.between(0.1, 0.9);
                p.y = height + 5.0;
                let rise = p.y - height * r.between(0.12, 0.5);
                p.vy = -(2.0 * GRAVITY * rise).sqrt();
                p.vx = r.between(-20.0, 20.0);
                p.color = r.pick(&FIREWORKS);
            }
            Season::June => {}
        }
        p
    }

    /// Moves everything `dt` seconds on.
    fn step(&mut self, dt: f64, width: f64, height: f64) {
        if width <= 0.0 {
            return;
        }
        self.t += dt;
        let t = self.t;
        let wanted = self.population(width, height);
        if !self.filled {
            self.filled = true;
            for _ in 0..wanted {
                let particle = self.spawn(width, height, true);
                self.particles.push(particle);
            }
        }
        let living = self.particles.iter().filter(|p| p.shape != Shape::Spark && p.shape != Shape::Rocket).count();
        if living < wanted {
            let particle = self.spawn(width, height, false);
            self.particles.push(particle);
        }
        if self.season == Season::NewYear {
            self.owed += 1.1 * (width / 900.0).clamp(0.6, 2.0) * dt;
            while self.owed >= 1.0 {
                self.owed -= 1.0;
                let rocket = self.spawn(width, height, false);
                self.particles.push(rocket);
            }
        }
        for p in &mut self.particles {
            p.age += dt;
            let sway = (t * 2.0 + p.phase).sin();
            match p.shape {
                Shape::Confetti => {
                    p.x += sway * 25.0 * dt;
                    p.y += p.vy * dt;
                    p.angle += p.spin * dt;
                }
                Shape::Flake => {
                    p.x += (t * 1.3 + p.phase).sin() * 15.0 * dt;
                    p.y += p.vy * dt;
                }
                Shape::Egg => {
                    p.y += p.vy * dt;
                    p.angle = (t * 2.0 + p.phase).sin() * 0.3;
                }
                Shape::Heart => {
                    p.x += sway * 18.0 * dt;
                    p.y += p.vy * dt;
                }
                Shape::Bat => fly(p, t, dt, width, height),
                Shape::Rocket => {
                    p.vy += GRAVITY * dt;
                    p.x += p.vx * dt;
                    p.y += p.vy * dt;
                }
                Shape::Spark => {
                    let drag = 1.0 - 1.2 * dt;
                    p.vx *= drag;
                    p.vy = p.vy * drag + 60.0 * dt;
                    p.x += p.vx * dt;
                    p.y += p.vy * dt;
                }
            }
        }
        // Rockets at the top of their climb burst into sparks.
        let mut bursts = Vec::new();
        self.particles.retain(|p| {
            let gone = match p.shape {
                Shape::Rocket if p.vy >= 0.0 => {
                    bursts.push((p.x, p.y, p.color));
                    true
                }
                Shape::Heart => p.y < -30.0,
                Shape::Bat => p.x < -80.0 || p.x > width + 80.0 || p.y < -80.0 || p.y > height + 80.0,
                Shape::Spark => p.age >= p.life,
                _ => p.y > height + 30.0,
            };
            !gone
        });
        for (x, y, color) in bursts {
            let reach = (width.min(height) / 600.0).clamp(0.6, 1.4);
            for k in 0..36u32 {
                let r = &mut self.random;
                let direction = f64::from(k) * 2.0 * PI / 36.0 + r.between(-0.08, 0.08);
                let speed = r.between(60.0, 150.0) * reach;
                self.particles.push(Particle {
                    shape: Shape::Spark,
                    x,
                    y,
                    vx: direction.cos() * speed,
                    vy: direction.sin() * speed,
                    angle: 0.0,
                    spin: 0.0,
                    size: 2.2,
                    color,
                    trim: color,
                    phase: 0.0,
                    age: 0.0,
                    life: r.between(1.1, 1.6),
                });
            }
        }
    }

    fn draw(&self, cr: &cairo::Context, width: f64, height: f64) {
        let t = self.t;
        match self.season {
            Season::Carnival => {
                let radius = width.hypot(height);
                radial(cr, width * 0.15, 0.0, radius, &[(0.0, "#5b21b6"), (0.45, "#2a0f4d"), (1.0, "#14061f")], width, height);
                serpentines(cr, width);
            }
            Season::Easter => vertical(cr, width, height, &[(0.0, "#33200f"), (1.0, "#1a0f0a")]),
            Season::Valentines => {
                vertical(cr, width, height, &[(0.0, "#120a14"), (1.0, "#120a14")]);
                neon_heart(cr, width, height, t);
            }
            Season::June => {
                vertical(cr, width, height, &[(0.0, "#0a1533"), (1.0, "#1d2f63")]);
                self.draw_stars(cr, width, height, (1.0, 0.976, 0.765));
                bunting(cr, width, t);
            }
            Season::Halloween => {
                vertical(cr, width, height, &[(0.0, "#34423c"), (1.0, "#18211d")]);
                moon(cr, width - 275.0, 85.0, 45.0);
                fog(cr, width, height, t, &[(420.0, 200.0, 300.0, 60.0, 0.08, 50.0)]);
                tree(cr, width, height);
            }
            Season::Christmas => vertical(cr, width, height, &[(0.0, "#0a1730"), (0.65, "#12274a"), (1.0, "#1c3760")]),
            Season::NewYear => {
                vertical(cr, width, height, &[(0.0, "#04060d"), (0.7, "#0a0f22"), (1.0, "#161d3a")]);
                self.draw_stars(cr, width, height, (1.0, 1.0, 1.0));
            }
        }
        for p in &self.particles {
            draw_particle(cr, p, t);
        }
        match self.season {
            Season::Easter => bunny(cr, width, height, t),
            Season::Halloween => fog(cr, width, height, t, &[(280.0, -120.0, 380.0, 80.0, 0.13, 32.0), (670.0, -65.0, 410.0, 95.0, 0.16, 42.0)]),
            Season::Christmas => {
                hills(cr, width, height);
                fairy_lights(cr, width, t);
            }
            Season::NewYear => self.draw_skyline(cr, width, height),
            _ => {}
        }
    }

    fn draw_stars(&self, cr: &cairo::Context, width: f64, height: f64, (r, g, b): (f64, f64, f64)) {
        for &(x, y, size, period, phase) in &self.stars {
            let alpha = 0.15 + 0.85 * (0.5 + 0.5 * (self.t * 2.0 * PI / period + phase).sin());
            cr.set_source_rgba(r, g, b, alpha);
            cr.arc(x * width, y * height, size, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
    }

    fn draw_skyline(&self, cr: &cairo::Context, width: f64, height: f64) {
        cr.set_source_rgb(0.012, 0.02, 0.043);
        let mut x = 0.0;
        for &(w, h) in &self.buildings {
            if x >= width {
                break;
            }
            cr.rectangle(x, height - h, w, h);
            x += w + 2.0;
        }
        let _ = cr.fill();
    }
}

/// A bat's flight: its heading wanders, it bobs up and down, and until its
/// time is up it turns back in when it nears an edge.
fn fly(p: &mut Particle, t: f64, dt: f64, width: f64, height: f64) {
    let turn = (t * 0.7 + p.phase).sin() * 0.9 + (t * 1.9 + p.phase * 2.0).sin() * 0.5;
    p.angle += turn * dt;
    let near_edge = p.x < 60.0 || p.x > width - 60.0 || p.y < 30.0 || p.y > height * 0.8;
    if p.age < p.life && near_edge {
        let inward = (height * 0.4 - p.y).atan2(width * 0.5 - p.x);
        let off = (inward - p.angle + PI).rem_euclid(2.0 * PI) - PI;
        p.angle += off * 1.6 * dt;
    }
    p.vx = p.angle.cos() * p.spin;
    p.vy = p.angle.sin() * p.spin * 0.6 + (t * 2.3 + p.phase).sin() * 16.0;
    p.x += p.vx * dt;
    p.y += p.vy * dt;
}

/// A vertical gradient over the whole area.
fn vertical(cr: &cairo::Context, width: f64, height: f64, stops: &[(f64, &str)]) {
    let gradient = cairo::LinearGradient::new(0.0, 0.0, 0.0, height);
    for &(at, hex) in stops {
        let (r, g, b) = theme::rgb(hex);
        gradient.add_color_stop_rgb(at, r, g, b);
    }
    let _ = cr.set_source(&gradient);
    cr.rectangle(0.0, 0.0, width, height);
    let _ = cr.fill();
}

/// A circular gradient from (x, y) over the whole area.
fn radial(cr: &cairo::Context, x: f64, y: f64, radius: f64, stops: &[(f64, &str)], width: f64, height: f64) {
    let gradient = cairo::RadialGradient::new(x, y, 0.0, x, y, radius);
    for &(at, hex) in stops {
        let (r, g, b) = theme::rgb(hex);
        gradient.add_color_stop_rgb(at, r, g, b);
    }
    let _ = cr.set_source(&gradient);
    cr.rectangle(0.0, 0.0, width, height);
    let _ = cr.fill();
}

/// A soft round glow: `color` at `alpha` in the middle, nothing at `radius`.
fn glow(cr: &cairo::Context, x: f64, y: f64, radius: f64, (r, g, b): (f64, f64, f64), alpha: f64) {
    let gradient = cairo::RadialGradient::new(x, y, 0.0, x, y, radius);
    gradient.add_color_stop_rgba(0.0, r, g, b, alpha);
    gradient.add_color_stop_rgba(1.0, r, g, b, 0.0);
    let _ = cr.set_source(&gradient);
    cr.arc(x, y, radius, 0.0, 2.0 * PI);
    let _ = cr.fill();
}

/// A quadratic curve (cairo draws cubic ones) from the current point.
fn quad_to(cr: &cairo::Context, (x0, y0): (f64, f64), (cx, cy): (f64, f64), (x, y): (f64, f64)) {
    cr.curve_to(x0 + 2.0 / 3.0 * (cx - x0), y0 + 2.0 / 3.0 * (cy - y0), x + 2.0 / 3.0 * (cx - x), y + 2.0 / 3.0 * (cy - y), x, y);
}

/// The point at `t` (0–1) of a quadratic curve.
fn quad_at((x0, y0): (f64, f64), (cx, cy): (f64, f64), (x, y): (f64, f64), t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    (u * u * x0 + 2.0 * t * u * cx + t * t * x, u * u * y0 + 2.0 * t * u * cy + t * t * y)
}

/// Carnaval's paper streamers, hanging from the top.
fn serpentines(cr: &cairo::Context, width: f64) {
    let sx = width / 960.0;
    let streamers: [(&str, [f64; 14]); 4] = [
        ("#f472b6", [230.0, 0.0, 260.0, 40.0, 200.0, 60.0, 236.0, 96.0, 270.0, 130.0, 214.0, 150.0, 246.0, 190.0]),
        ("#facc15", [300.0, 0.0, 330.0, 30.0, 286.0, 54.0, 318.0, 80.0, 350.0, 104.0, 304.0, 122.0, 330.0, 150.0]),
        ("#34d399", [700.0, 0.0, 672.0, 36.0, 728.0, 58.0, 694.0, 92.0, 662.0, 124.0, 716.0, 146.0, 690.0, 180.0]),
        ("#60a5fa", [820.0, 0.0, 792.0, 26.0, 840.0, 50.0, 808.0, 76.0, 778.0, 100.0, 826.0, 118.0, 800.0, 140.0]),
    ];
    cr.set_line_width(3.5);
    cr.set_line_cap(cairo::LineCap::Round);
    for (hex, c) in streamers {
        let (r, g, b) = theme::rgb(hex);
        cr.set_source_rgba(r, g, b, 0.72);
        cr.move_to(c[0] * sx, c[1]);
        cr.curve_to(c[2] * sx, c[3], c[4] * sx, c[5], c[6] * sx, c[7]);
        cr.curve_to(c[8] * sx, c[9], c[10] * sx, c[11], c[12] * sx, c[13]);
        let _ = cr.stroke();
    }
}

/// A heart in a 24×24 box, as a path.
fn heart_path(cr: &cairo::Context) {
    cr.move_to(12.0, 20.5);
    cr.curve_to(12.0, 20.5, 5.0, 16.1, 2.6, 11.8);
    cr.curve_to(0.9, 8.4, 2.7, 4.6, 6.4, 4.6);
    cr.curve_to(8.5, 4.6, 10.0, 5.8, 12.0, 7.9);
    cr.curve_to(14.0, 5.8, 15.5, 4.6, 17.6, 4.6);
    cr.curve_to(21.3, 4.6, 23.1, 8.4, 21.4, 11.8);
    cr.curve_to(19.0, 16.1, 12.0, 20.5, 12.0, 20.5);
    cr.close_path();
}

/// The Dia dos Namorados' big heart, an outline of neon that pulses.
fn neon_heart(cr: &cairo::Context, width: f64, height: f64, t: f64) {
    let size = (width * 0.4).min(height * 0.72);
    let scale = size / 24.0;
    let pulse = 0.45 + 0.55 * (0.5 + 0.5 * (t * 2.0 * PI / 4.4).sin());
    let (r, g, b) = theme::rgb("#ff4d8d");
    let _ = cr.save();
    cr.translate(width * 0.615 - size / 2.0, height * 0.53 - size / 2.0);
    cr.scale(scale, scale);
    for (line, alpha) in [(18.0, 0.05), (10.0, 0.11), (5.0, 0.25), (2.0, 1.0)] {
        heart_path(cr);
        cr.set_line_width(line / scale);
        cr.set_source_rgba(r, g, b, alpha * pulse);
        let _ = cr.stroke();
    }
    let _ = cr.restore();
}

/// Festa junina's bandeirinhas: two strings across the top, swaying.
fn bunting(cr: &cairo::Context, width: f64, t: f64) {
    let _ = cr.save();
    cr.translate(width / 2.0, 0.0);
    cr.rotate((1.2f64).to_radians() * (t * 2.0 * PI / 6.4).sin());
    cr.translate(-width / 2.0, 0.0);
    for (string, (top, sag, offset)) in [(8.0, 60.0, 0.5), (46.0, 100.0, 0.0)].into_iter().enumerate() {
        let (start, control, end) = ((0.0, top), (width / 2.0, sag), (width, top));
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.35);
        cr.set_line_width(1.2);
        cr.move_to(start.0, start.1);
        quad_to(cr, start, control, end);
        let _ = cr.stroke();
        let count = ((width / 37.0).round() as usize).max(8);
        for i in 0..count {
            let (x, y) = quad_at(start, control, end, (i as f64 + offset) / count as f64);
            let (r, g, b) = theme::rgb(FLAGS[(i + string * 3) % FLAGS.len()]);
            cr.set_source_rgb(r, g, b);
            cr.move_to(x - 9.0, y);
            cr.line_to(x + 9.0, y);
            cr.line_to(x + 9.0, y + 22.0);
            cr.line_to(x, y + 16.0);
            cr.line_to(x - 9.0, y + 22.0);
            cr.close_path();
            let _ = cr.fill();
        }
    }
    let _ = cr.restore();
}

/// Halloween's pale green moon.
fn moon(cr: &cairo::Context, x: f64, y: f64, radius: f64) {
    glow(cr, x, y, radius + 70.0, (0.84, 0.93, 0.75), 0.16);
    let gradient = cairo::RadialGradient::new(x - radius * 0.2, y - radius * 0.2, 0.0, x, y, radius);
    gradient.add_color_stop_rgb(0.0, 0.945, 0.969, 0.902);
    gradient.add_color_stop_rgb(1.0, 0.812, 0.878, 0.733);
    let _ = cr.set_source(&gradient);
    cr.arc(x, y, radius, 0.0, 2.0 * PI);
    let _ = cr.fill();
}

/// Banks of fog drifting sideways: centre (x of a 960-wide window; y from
/// the top, or from the bottom when negative), radii, strength and period.
fn fog(cr: &cairo::Context, width: f64, height: f64, t: f64, banks: &[(f64, f64, f64, f64, f64, f64)]) {
    let sx = width / 960.0;
    for &(x, y, rx, ry, alpha, period) in banks {
        let drift = (t * 2.0 * PI / period).sin() * 140.0;
        let y = if y < 0.0 { height + y } else { y };
        let _ = cr.save();
        cr.translate(x * sx + drift, y);
        cr.scale(rx * sx, ry);
        let gradient = cairo::RadialGradient::new(0.0, 0.0, 0.0, 0.0, 0.0, 1.0);
        gradient.add_color_stop_rgba(0.0, 0.84, 0.91, 0.87, alpha);
        gradient.add_color_stop_rgba(0.7, 0.84, 0.91, 0.87, 0.0);
        let _ = cr.set_source(&gradient);
        cr.arc(0.0, 0.0, 1.0, 0.0, 2.0 * PI);
        let _ = cr.fill();
        let _ = cr.restore();
    }
}

/// A bare tree at the bottom right.
fn tree(cr: &cairo::Context, width: f64, height: f64) {
    let _ = cr.save();
    cr.translate(width + 10.0 - 220.0, height - 330.0);
    cr.scale(1.1, 1.1);
    cr.set_source_rgb(0.027, 0.039, 0.035);
    cr.set_line_cap(cairo::LineCap::Round);
    let branches: [(f64, [f64; 8]); 6] = [
        (7.0, [116.0, 150.0, 90.0, 130.0, 70.0, 132.0, 40.0, 110.0]),
        (4.0, [60.0, 122.0, 50.0, 100.0, 52.0, 86.0, 40.0, 70.0]),
        (6.0, [118.0, 110.0, 140.0, 90.0, 160.0, 92.0, 186.0, 70.0]),
        (3.5, [164.0, 86.0, 170.0, 66.0, 168.0, 52.0, 178.0, 36.0]),
        (5.0, [112.0, 60.0, 100.0, 40.0, 104.0, 26.0, 92.0, 8.0]),
        (3.5, [113.0, 80.0, 130.0, 60.0, 128.0, 44.0, 142.0, 30.0]),
    ];
    cr.set_line_width(16.0);
    cr.move_to(120.0, 300.0);
    cr.curve_to(118.0, 240.0, 126.0, 200.0, 116.0, 150.0);
    cr.curve_to(110.0, 120.0, 118.0, 90.0, 112.0, 60.0);
    let _ = cr.stroke();
    for (line, c) in branches {
        cr.set_line_width(line);
        cr.move_to(c[0], c[1]);
        cr.curve_to(c[2], c[3], c[4], c[5], c[6], c[7]);
        let _ = cr.stroke();
    }
    let _ = cr.restore();
}

/// Easter's bunny, peeking from the bottom right, its ears twitching.
fn bunny(cr: &cairo::Context, width: f64, height: f64, t: f64) {
    let _ = cr.save();
    cr.translate(width - 70.0 - 150.0, height - 138.0);
    cr.scale(1.25, 1.25);
    let twitch = (t * 1.5).sin() * 4.0;
    let ellipse = |cx: f64, cy: f64, rx: f64, ry: f64, degrees: f64| {
        let _ = cr.save();
        cr.translate(cx, cy);
        cr.rotate(degrees.to_radians());
        cr.scale(rx, ry);
        cr.arc(0.0, 0.0, 1.0, 0.0, 2.0 * PI);
        let _ = cr.restore();
    };
    cr.set_source_rgb(0.29, 0.18, 0.12);
    ellipse(44.0, 34.0, 11.0, 32.0, -10.0 + twitch);
    ellipse(76.0, 34.0, 11.0, 32.0, 10.0 - twitch);
    ellipse(60.0, 92.0, 38.0, 32.0, 0.0);
    let _ = cr.fill();
    cr.set_source_rgba(0.976, 0.659, 0.831, 0.55);
    ellipse(44.0, 34.0, 5.0, 24.0, -10.0 + twitch);
    ellipse(76.0, 34.0, 5.0, 24.0, 10.0 - twitch);
    let _ = cr.fill();
    cr.set_source_rgb(0.1, 0.06, 0.04);
    cr.arc(47.0, 86.0, 3.5, 0.0, 2.0 * PI);
    let _ = cr.fill();
    cr.arc(73.0, 86.0, 3.5, 0.0, 2.0 * PI);
    let _ = cr.fill();
    cr.set_source_rgb(0.976, 0.659, 0.831);
    ellipse(60.0, 96.0, 4.0, 3.0, 0.0);
    let _ = cr.fill();
    let _ = cr.restore();
}

/// Christmas: snowy hills with pines at the bottom.
fn hills(cr: &cairo::Context, width: f64, height: f64) {
    let _ = cr.save();
    cr.translate(0.0, height - 120.0);
    let sx = width / 960.0;
    cr.set_source_rgb(0.043, 0.094, 0.188);
    // Pines: left foot, top and right foot (x), base and top (y).
    let pines: [(f64, f64, f64, f64, f64); 5] = [
        (140.0, 160.0, 180.0, 60.0, 22.0),
        (168.0, 186.0, 204.0, 64.0, 30.0),
        (610.0, 626.0, 642.0, 66.0, 36.0),
        (700.0, 722.0, 744.0, 54.0, 12.0),
        (732.0, 750.0, 768.0, 60.0, 26.0),
    ];
    for (left, middle, right, base, top) in pines {
        cr.move_to(left * sx, base);
        cr.line_to(middle * sx, top);
        cr.line_to(right * sx, base);
        cr.close_path();
    }
    let _ = cr.fill();
    let (r, g, b) = theme::rgb("#e6eef8");
    cr.set_source_rgba(r, g, b, 0.92);
    cr.move_to(0.0, 70.0);
    cr.curve_to(120.0 * sx, 40.0, 220.0 * sx, 52.0, 330.0 * sx, 74.0);
    cr.curve_to(450.0 * sx, 98.0, 560.0 * sx, 50.0, 690.0 * sx, 56.0);
    cr.curve_to(800.0 * sx, 60.0, 880.0 * sx, 80.0, width, 66.0);
    cr.line_to(width, 120.0);
    cr.line_to(0.0, 120.0);
    cr.close_path();
    let _ = cr.fill();
    let (r, g, b) = theme::rgb("#f5f9fd");
    cr.set_source_rgb(r, g, b);
    cr.move_to(0.0, 92.0);
    cr.curve_to(160.0 * sx, 76.0, 300.0 * sx, 100.0, 460.0 * sx, 96.0);
    cr.curve_to(620.0 * sx, 92.0, 760.0 * sx, 74.0, width, 90.0);
    cr.line_to(width, 120.0);
    cr.line_to(0.0, 120.0);
    cr.close_path();
    let _ = cr.fill();
    let _ = cr.restore();
}

/// Christmas: a string of coloured lights across the top, twinkling.
fn fairy_lights(cr: &cairo::Context, width: f64, t: f64) {
    let half = width / 2.0;
    cr.set_source_rgb(0.016, 0.078, 0.039);
    cr.set_line_width(2.0);
    cr.move_to(0.0, 6.0);
    quad_to(cr, (0.0, 6.0), (half / 2.0, 56.0), (half, 6.0));
    quad_to(cr, (half, 6.0), (half * 1.5, 56.0), (width, 6.0));
    let _ = cr.stroke();
    let per_half = ((half / 40.0).round() as usize).max(6);
    for segment in 0..2 {
        let start = (half * segment as f64, 6.0);
        let control = (start.0 + half / 2.0, 56.0);
        let end = (start.0 + half, 6.0);
        for i in 0..per_half {
            let (x, y) = quad_at(start, control, end, (i as f64 + 0.5) / per_half as f64);
            let color = theme::rgb(BULBS[(i + segment) % BULBS.len()]);
            let on = 0.15 + 0.85 * (0.5 + 0.5 * (t * 2.0 * PI / 1.8 + i as f64 * 1.3 + segment as f64).sin());
            glow(cr, x, y + 7.0, 13.0, color, 0.55 * on);
            let _ = cr.save();
            cr.translate(x, y + 7.0);
            cr.scale(4.0, 6.0);
            cr.arc(0.0, 0.0, 1.0, 0.0, 2.0 * PI);
            let _ = cr.restore();
            cr.set_source_rgba(color.0, color.1, color.2, 0.35 + 0.65 * on);
            let _ = cr.fill();
        }
    }
}

fn draw_particle(cr: &cairo::Context, p: &Particle, t: f64) {
    let _ = cr.save();
    cr.translate(p.x, p.y);
    let (r, g, b) = p.color;
    cr.set_source_rgba(r, g, b, 0.92);
    match p.shape {
        Shape::Confetti => {
            // Turning over: it narrows and widens.
            cr.rotate(p.angle);
            cr.scale(1.0, (t * 5.0 + p.phase).cos().abs().max(0.2));
            cr.rectangle(-p.size / 2.0, -p.size / 4.0, p.size, p.size / 2.0);
            let _ = cr.fill();
        }
        Shape::Flake => {
            cr.set_source_rgba(r, g, b, 0.85);
            cr.arc(0.0, 0.0, p.size, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
        Shape::Egg => {
            cr.rotate(p.angle);
            draw_egg(cr, p);
        }
        Shape::Heart => {
            cr.set_source_rgba(r, g, b, 0.8);
            cr.scale(p.size / 24.0, p.size / 24.0);
            cr.translate(-12.0, -12.5);
            heart_path(cr);
            let _ = cr.fill();
        }
        Shape::Bat => {
            // Tilted with its climb or dive.
            cr.rotate((p.vy / p.spin.max(1.0)).clamp(-0.5, 0.5) * 0.6);
            draw_bat(cr, p.size, (t * 13.0 + p.phase).sin());
        }
        Shape::Rocket => {
            cr.set_line_width(2.0);
            cr.move_to(0.0, 0.0);
            cr.line_to(-p.vx * 0.06, -p.vy * 0.06);
            let _ = cr.stroke();
            cr.arc(0.0, 0.0, 2.2, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
        Shape::Spark => {
            cr.set_source_rgba(r, g, b, (1.0 - p.age / p.life).clamp(0.0, 1.0));
            cr.arc(0.0, 0.0, p.size, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
    }
    let _ = cr.restore();
}

/// A painted egg: a zigzag band and dots in its second color. It clips to
/// the egg, which the caller's `restore` undoes (never `reset_clip`: that
/// would also lift GTK's clip to the widget).
fn draw_egg(cr: &cairo::Context, p: &Particle) {
    let s = p.size;
    let _ = cr.save();
    cr.scale(0.78, 1.0);
    cr.arc(0.0, 0.0, s, 0.0, 2.0 * PI);
    let _ = cr.restore();
    let _ = cr.fill_preserve();
    cr.clip();
    let (r, g, b) = p.trim;
    cr.set_source_rgba(r, g, b, 0.95);
    let step = s / 3.0;
    cr.move_to(-s, -s * 0.05);
    let mut x = -s;
    let mut up = true;
    while x < s {
        x += step;
        cr.line_to(x, if up { -s * 0.2 } else { -s * 0.05 });
        up = !up;
    }
    cr.line_to(s, s * 0.25);
    while x > -s {
        x -= step;
        cr.line_to(x, if up { s * 0.1 } else { s * 0.25 });
        up = !up;
    }
    cr.close_path();
    let _ = cr.fill();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.8);
    for (x, y) in [(-0.3, -0.5), (0.3, -0.5), (0.0, 0.6)] {
        cr.arc(x * s, y * s, s * 0.1, 0.0, 2.0 * PI);
        let _ = cr.fill();
    }
}

/// A bat facing the viewer, its wings at `flap` (−1 down to 1 up).
fn draw_bat(cr: &cairo::Context, s: f64, flap: f64) {
    let lift = flap * s * 0.4;
    for side in [1.0, -1.0] {
        let _ = cr.save();
        cr.scale(side, 1.0);
        cr.move_to(s * 0.08, -s * 0.08);
        cr.curve_to(s * 0.35, -s * 0.2 - lift, s * 0.75, -s * 0.3 - lift, s, -s * 0.05 - lift);
        // The scalloped edge, back to the body.
        cr.curve_to(s * 0.85, -lift * 0.6, s * 0.75, s * 0.05 - lift * 0.5, s * 0.6, s * 0.02 - lift * 0.4);
        cr.curve_to(s * 0.5, s * 0.12 - lift * 0.3, s * 0.35, s * 0.1 - lift * 0.2, s * 0.25, s * 0.06);
        cr.line_to(s * 0.08, s * 0.12);
        cr.close_path();
        let _ = cr.fill();
        // An ear.
        cr.move_to(s * 0.03, -s * 0.15);
        cr.line_to(s * 0.1, -s * 0.3);
        cr.line_to(s * 0.12, -s * 0.12);
        cr.close_path();
        let _ = cr.fill();
        let _ = cr.restore();
    }
    let _ = cr.save();
    cr.scale(0.12, 0.2);
    cr.arc(0.0, 0.0, s, 0.0, 2.0 * PI);
    let _ = cr.restore();
    let _ = cr.fill();
}

/// Whether the window is on screen: not minimized, hidden or (on Wayland)
/// suspended.
fn on_screen(area: &gtk::DrawingArea) -> bool {
    let Some(window) = area.root().and_downcast::<gtk::Window>() else { return false };
    if !window.is_visible() || window.is_suspended() {
        return false;
    }
    let minimized = window
        .surface()
        .and_downcast::<gdk::Toplevel>()
        .is_some_and(|toplevel| toplevel.state().contains(gdk::ToplevelState::MINIMIZED));
    !minimized
}

/// The layer behind the main window's panels.
pub struct Effects {
    pub area: gtk::DrawingArea,
    scene: Rc<RefCell<Option<Scene>>>,
    ticking: Cell<bool>,
}

thread_local! {
    /// The main window's layer, which `show` changes.
    static LAYER: RefCell<Weak<Effects>> = const { RefCell::new(Weak::new()) };
}

impl Effects {
    pub fn new() -> Rc<Self> {
        let area = gtk::DrawingArea::builder().can_target(false).can_focus(false).visible(false).build();
        let scene: Rc<RefCell<Option<Scene>>> = Rc::default();
        let s = scene.clone();
        area.set_draw_func(move |_, cr, width, height| {
            if let Some(scene) = s.borrow_mut().as_mut() {
                let (width, height) = (f64::from(width), f64::from(height));
                // Still (animations off): the scene as it starts.
                if !scene.filled {
                    scene.step(0.0, width, height);
                }
                scene.draw(cr, width, height);
            }
        });
        let effects = Rc::new(Self { area, scene, ticking: Cell::new(false) });
        LAYER.replace(Rc::downgrade(&effects));
        effects.set(theme::season());
        effects
    }

    /// Shows a holiday's scene, or none.
    fn set(self: &Rc<Self>, season: Option<Season>) {
        if self.scene.borrow().as_ref().map(|scene| scene.season) == season {
            return;
        }
        self.scene.replace(season.map(|season| Scene::new(season, glib::real_time() as u64)));
        self.area.set_visible(season.is_some());
        self.area.queue_draw();
        let animations = gtk::Settings::default().is_none_or(|settings| settings.is_gtk_enable_animations());
        if season.is_none() || !animations || self.ticking.replace(true) {
            return;
        }
        let this = Rc::downgrade(self);
        self.area.add_tick_callback(move |area, clock| {
            let Some(this) = this.upgrade() else { return glib::ControlFlow::Break };
            let mut scene = this.scene.borrow_mut();
            let Some(scene) = scene.as_mut() else {
                this.ticking.set(false);
                return glib::ControlFlow::Break;
            };
            if !on_screen(area) {
                // Paused: when it is back, it goes on from where it was.
                scene.last = None;
                return glib::ControlFlow::Continue;
            }
            let now = clock.frame_time();
            let dt = match scene.last {
                None => 0.0,
                Some(last) if now - last < FRAME => return glib::ControlFlow::Continue,
                Some(last) => ((now - last) as f64 / 1e6).min(0.1),
            };
            scene.last = Some(now);
            scene.step(dt, f64::from(area.width()), f64::from(area.height()));
            area.queue_draw();
            glib::ControlFlow::Continue
        });
    }
}

/// The holiday's scene behind the main window (if it is open), or none.
pub fn show(season: Option<Season>) {
    if let Some(effects) = LAYER.with_borrow(Weak::upgrade) {
        effects.set(season);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easter_dates() {
        assert_eq!(easter(2024), (3, 31));
        assert_eq!(easter(2025), (4, 20));
        assert_eq!(easter(2026), (4, 5));
        assert_eq!(easter(2027), (3, 28));
        assert_eq!(easter(2038), (4, 25));
    }

    #[test]
    fn day_numbers() {
        assert_eq!(day_number(1970, 1, 1), 0);
        assert_eq!(day_number(2000, 3, 1) - day_number(2000, 2, 28), 2);
        assert_eq!(day_number(2027, 1, 1) - day_number(2026, 12, 31), 1);
    }

    #[test]
    fn holidays_of_the_year() {
        // Carnaval 2026: Tuesday 17 February; from Friday 13 to Ash Wednesday.
        assert_eq!(Season::on(2026, 2, 12), None);
        assert_eq!(Season::on(2026, 2, 13), Some(Season::Carnival));
        assert_eq!(Season::on(2026, 2, 17), Some(Season::Carnival));
        assert_eq!(Season::on(2026, 2, 18), Some(Season::Carnival));
        assert_eq!(Season::on(2026, 2, 19), None);
        // Carnaval 2027, in another month's days: Tuesday 9 February.
        assert_eq!(Season::on(2027, 2, 9), Some(Season::Carnival));
        assert_eq!(Season::on(2026, 3, 28), None);
        assert_eq!(Season::on(2026, 3, 29), Some(Season::Easter));
        assert_eq!(Season::on(2026, 4, 5), Some(Season::Easter));
        assert_eq!(Season::on(2026, 4, 6), None);
        assert_eq!(Season::on(2026, 6, 12), Some(Season::Valentines));
        assert_eq!(Season::on(2026, 6, 24), Some(Season::June));
        assert_eq!(Season::on(2026, 10, 29), Some(Season::Halloween));
        assert_eq!(Season::on(2026, 11, 1), None);
        assert_eq!(Season::on(2026, 12, 25), Some(Season::Christmas));
        assert_eq!(Season::on(2026, 12, 31), Some(Season::NewYear));
        assert_eq!(Season::on(2027, 1, 2), Some(Season::NewYear));
        assert_eq!(Season::on(2027, 1, 3), None);
        assert_eq!(Season::on(2026, 7, 15), None);
    }

    #[test]
    fn every_scene_draws() {
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 1000, 600).unwrap();
        let cr = cairo::Context::new(&surface).unwrap();
        for (seed, season) in Season::ALL.into_iter().enumerate() {
            let mut scene = Scene::new(season, seed as u64 + 3);
            for _ in 0..90 {
                scene.step(1.0 / 30.0, 1000.0, 600.0);
                scene.draw(&cr, 1000.0, 600.0);
            }
            assert_eq!(cr.status(), Ok(()), "{season:?}");
        }
    }

    #[test]
    fn scenes_stay_full_and_bounded() {
        for (seed, season) in Season::ALL.into_iter().enumerate() {
            let mut scene = Scene::new(season, seed as u64 * 7919 + 1);
            // Ten minutes at 30 frames per second.
            for _ in 0..30 * 600 {
                scene.step(1.0 / 30.0, 1000.0, 600.0);
            }
            let wanted = scene.population(1000.0, 600.0);
            let living = scene.particles.iter().filter(|p| p.shape != Shape::Spark && p.shape != Shape::Rocket).count();
            assert!(living <= wanted, "{season:?}: {living} of {wanted}");
            assert!(living + 3 >= wanted, "{season:?}: {living} of {wanted}");
            assert!(scene.particles.len() < 600, "{season:?}: {} particles", scene.particles.len());
            // The bats stay around, mostly inside the window.
            if season == Season::Halloween {
                let inside = scene.particles.iter().filter(|p| (0.0..1000.0).contains(&p.x) && (0.0..600.0).contains(&p.y)).count();
                assert!(inside * 2 >= living, "{inside} of {living} bats inside");
            }
        }
    }
}
