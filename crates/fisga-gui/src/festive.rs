//! The holidays. In their weeks, the main window opens with an effect over
//! it for some seconds (confetti in Carnaval, eggs at Easter, bats at
//! Halloween, snow at Christmas, fireworks at New Year...) and the accent
//! takes the holiday's color. The effect is drawn with cairo on a layer that
//! lets the clicks through; "Testar" in the preferences plays any of them.

use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::rc::{Rc, Weak};

use gtk::glib;
use gtk::prelude::*;

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
            Season::Carnival => "Carnaval: confete",
            Season::Easter => "Páscoa: ovos",
            Season::Valentines => "Dia dos Namorados: corações",
            Season::June => "Festa junina: bandeirinhas",
            Season::Halloween => "Halloween: morcegos",
            Season::Christmas => "Natal: neve",
            Season::NewYear => "Ano-novo: fogos",
        }
    }

    /// The accent, and the accent as text on the dark and on the light theme.
    pub fn accent(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Season::Carnival => ("#d946ef", "#e879f9", "#a21caf"),
            Season::Easter => ("#a78bfa", "#c4b5fd", "#6d28d9"),
            Season::Valentines => ("#f472b6", "#f9a8d4", "#be185d"),
            Season::June => ("#fb923c", "#fdba74", "#c2410c"),
            Season::Halloween => ("#a855f7", "#c084fc", "#7e22ce"),
            Season::Christmas => ("#16a34a", "#4ade80", "#15803d"),
            Season::NewYear => ("#facc15", "#fde047", "#a16207"),
        }
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

    pub fn today() -> Option<Season> {
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

/// Seconds during which new particles come in; the last ones then leave.
const SPAWNING: f64 = 15.0;
/// What pulls the rockets down, in pixels/s².
const GRAVITY: f64 = 220.0;

const CONFETTI: [&str; 7] = ["#f472b6", "#facc15", "#34d399", "#60a5fa", "#a78bfa", "#fb923c", "#f87171"];
const PASTELS: [&str; 6] = ["#f9a8d4", "#a5f3fc", "#fde68a", "#c4b5fd", "#bbf7d0", "#fecaca"];
const HEARTS: [&str; 4] = ["#fb7185", "#f43f5e", "#fda4af", "#e11d48"];
const FLAGS: [&str; 7] = ["#ef4444", "#facc15", "#22c55e", "#3b82f6", "#f97316", "#a855f7", "#ec4899"];
const FIREWORKS: [&str; 7] = ["#facc15", "#f87171", "#60a5fa", "#34d399", "#f472b6", "#c084fc", "#fb923c"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Confetti,
    Flake,
    Egg,
    Heart,
    Flag,
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
    angle: f64,
    spin: f64,
    size: f64,
    color: (f64, f64, f64),
    /// A second color: an egg's stripe.
    trim: (f64, f64, f64),
    /// Where its sway (or wing beat) starts.
    phase: f64,
    /// Seconds lived, and the most it lives (sparks).
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

/// One effect being played.
struct Show {
    season: Season,
    /// Tried from the preferences: its colors go back at the end.
    preview: bool,
    /// Frame clock times (µs) of the first frame and of the last one.
    start: Option<i64>,
    last: i64,
    particles: Vec<Particle>,
    /// Particles owed by the spawn rate.
    owed: f64,
    random: Random,
}

impl Show {
    /// New particles per second, for a window 900 pixels wide.
    fn rate(&self) -> f64 {
        match self.season {
            Season::Carnival => 16.0,
            Season::Easter => 2.5,
            Season::Valentines => 3.0,
            Season::June => 9.0,
            Season::Halloween => 1.4,
            Season::Christmas => 20.0,
            Season::NewYear => 1.3,
        }
    }

    fn spawn(&mut self, width: f64, height: f64) -> Particle {
        let r = &mut self.random;
        let mut p = Particle {
            shape: Shape::Confetti,
            x: r.between(0.0, width),
            y: -20.0,
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
                p.vy = r.between(70.0, 140.0);
                p.angle = r.between(0.0, 2.0 * PI);
                p.spin = r.between(-6.0, 6.0);
                p.size = r.between(6.0, 10.0);
                p.color = r.pick(&CONFETTI);
            }
            Season::Easter => {
                p.shape = Shape::Egg;
                p.y = -30.0;
                p.vy = r.between(55.0, 90.0);
                p.size = r.between(10.0, 16.0);
                p.color = r.pick(&PASTELS);
                p.trim = r.pick(&PASTELS);
            }
            Season::Valentines => {
                p.shape = Shape::Heart;
                p.y = height + 20.0;
                p.vy = -r.between(40.0, 80.0);
                p.size = r.between(8.0, 14.0);
                p.color = r.pick(&HEARTS);
            }
            Season::June => {
                p.shape = Shape::Flag;
                p.vy = r.between(50.0, 90.0);
                p.size = r.between(9.0, 13.0);
                p.color = r.pick(&FLAGS);
            }
            Season::Halloween => {
                p.shape = Shape::Bat;
                let from_left = r.next() < 0.5;
                p.x = if from_left { -40.0 } else { width + 40.0 };
                p.y = height * r.between(0.1, 0.7);
                p.vx = r.between(110.0, 200.0) * if from_left { 1.0 } else { -1.0 };
                p.size = r.between(14.0, 22.0);
            }
            Season::Christmas => {
                p.shape = Shape::Flake;
                p.vy = r.between(30.0, 70.0);
                p.size = r.between(1.5, 4.0);
            }
            Season::NewYear => {
                // Climbs to a height between 15% and 45% of the window and bursts there.
                p.shape = Shape::Rocket;
                p.x = width * r.between(0.15, 0.85);
                p.y = height + 5.0;
                let rise = p.y - height * r.between(0.15, 0.45);
                p.vy = -(2.0 * GRAVITY * rise).sqrt();
                p.vx = r.between(-20.0, 20.0);
                p.color = r.pick(&FIREWORKS);
            }
        }
        p
    }

    /// Moves everything to the frame at `now`; false when the show is over.
    fn step(&mut self, now: i64, width: f64, height: f64) -> bool {
        let first = self.start.is_none();
        let start = *self.start.get_or_insert(now);
        let dt = if first { 0.0 } else { ((now - self.last) as f64 / 1e6).clamp(0.0, 0.05) };
        self.last = now;
        let t = (now - start) as f64 / 1e6;
        if t < SPAWNING && width > 0.0 {
            self.owed += self.rate() * (width / 900.0).clamp(0.6, 2.0) * dt;
            while self.owed >= 1.0 {
                self.owed -= 1.0;
                let particle = self.spawn(width, height);
                self.particles.push(particle);
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
                    p.angle = (t * 2.2 + p.phase).sin() * 0.35;
                }
                Shape::Heart => {
                    p.x += sway * 20.0 * dt;
                    p.y += p.vy * dt;
                }
                Shape::Flag => {
                    p.x += (t * 1.5 + p.phase).sin() * 30.0 * dt;
                    p.y += p.vy * dt;
                    p.angle = (t * 3.0 + p.phase).sin() * 0.5;
                }
                Shape::Bat => {
                    p.x += p.vx * dt;
                    p.y += sway * 30.0 * dt;
                }
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
                Shape::Bat => p.x < -60.0 || p.x > width + 60.0,
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
        // However it goes, a show ends: 25 s for the last particles to leave.
        (t < SPAWNING || !self.particles.is_empty()) && t < SPAWNING + 25.0
    }

    fn draw(&self, cr: &gtk::cairo::Context, height: f64) {
        let t = match self.start {
            Some(start) => (self.last - start) as f64 / 1e6,
            None => return,
        };
        let light = theme::is_light();
        for p in &self.particles {
            let _ = cr.save();
            cr.translate(p.x, p.y);
            cr.rotate(p.angle);
            let (r, g, b) = p.color;
            cr.set_source_rgba(r, g, b, 0.92);
            match p.shape {
                Shape::Confetti => {
                    // Turning over: it narrows and widens.
                    cr.scale(1.0, (t * 5.0 + p.phase).cos().abs().max(0.2));
                    cr.rectangle(-p.size / 2.0, -p.size / 4.0, p.size, p.size / 2.0);
                    let _ = cr.fill();
                }
                Shape::Flake => {
                    // White does not show on the light theme: a pale blue there.
                    let (r, g, b) = if light { (0.62, 0.74, 0.88) } else { (1.0, 1.0, 1.0) };
                    cr.set_source_rgba(r, g, b, 0.85);
                    cr.arc(0.0, 0.0, p.size, 0.0, 2.0 * PI);
                    let _ = cr.fill();
                }
                Shape::Egg => draw_egg(cr, p),
                Shape::Heart => {
                    // Fading out on the way up.
                    cr.set_source_rgba(r, g, b, (p.y / (height * 0.5)).clamp(0.0, 0.92));
                    let s = p.size;
                    cr.move_to(0.0, s * 0.35);
                    cr.curve_to(-s * 0.9, -s * 0.25, -s * 0.45, -s * 0.85, 0.0, -s * 0.35);
                    cr.curve_to(s * 0.45, -s * 0.85, s * 0.9, -s * 0.25, 0.0, s * 0.35);
                    let _ = cr.fill();
                }
                Shape::Flag => {
                    // A bandeirinha: cloth with a notch at the bottom.
                    let s = p.size;
                    cr.move_to(-s / 2.0, -s / 2.0);
                    cr.line_to(s / 2.0, -s / 2.0);
                    cr.line_to(s / 2.0, s / 2.0);
                    cr.line_to(0.0, s / 4.0);
                    cr.line_to(-s / 2.0, s / 2.0);
                    cr.close_path();
                    let _ = cr.fill();
                }
                Shape::Bat => {
                    let (r, g, b) = if light { (0.20, 0.10, 0.35) } else { (0.62, 0.52, 0.92) };
                    cr.set_source_rgba(r, g, b, 0.9);
                    draw_bat(cr, p.size, (t * 14.0 + p.phase).sin());
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
    }
}

/// A painted egg: a band and dots in its second color.
fn draw_egg(cr: &gtk::cairo::Context, p: &Particle) {
    let s = p.size;
    let _ = cr.save();
    cr.scale(0.78, 1.0);
    cr.arc(0.0, 0.0, s, 0.0, 2.0 * PI);
    let _ = cr.restore();
    let _ = cr.fill_preserve();
    cr.clip();
    let (r, g, b) = p.trim;
    cr.set_source_rgba(r, g, b, 0.95);
    cr.rectangle(-s, -s * 0.12, 2.0 * s, s * 0.24);
    let _ = cr.fill();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.8);
    for (x, y) in [(-0.3, -0.5), (0.3, -0.5), (0.0, 0.5)] {
        cr.arc(x * s, y * s, s * 0.1, 0.0, 2.0 * PI);
        let _ = cr.fill();
    }
    cr.reset_clip();
}

/// A bat facing the viewer, its wings at `flap` (−1 down to 1 up).
fn draw_bat(cr: &gtk::cairo::Context, s: f64, flap: f64) {
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

/// The layer over the main window.
pub struct Effects {
    pub area: gtk::DrawingArea,
    show: Rc<RefCell<Option<Show>>>,
    /// Whether the window already had its effect.
    celebrated: Cell<bool>,
    ticking: Cell<bool>,
}

thread_local! {
    /// The main window's layer, for "Testar" in the preferences.
    static LAYER: RefCell<Weak<Effects>> = RefCell::new(Weak::new());
}

impl Effects {
    pub fn new() -> Rc<Self> {
        let area = gtk::DrawingArea::builder().can_target(false).can_focus(false).visible(false).build();
        let show: Rc<RefCell<Option<Show>>> = Rc::default();
        let s = show.clone();
        area.set_draw_func(move |_, cr, _, height| {
            if let Some(show) = s.borrow().as_ref() {
                show.draw(cr, f64::from(height));
            }
        });
        let effects = Rc::new(Self { area, show, celebrated: Cell::new(false), ticking: Cell::new(false) });
        LAYER.replace(Rc::downgrade(&effects));
        effects
    }

    /// The holiday's effect, the first time the window asks (once its
    /// settings are known), unless the desktop turned animations off.
    pub fn celebrate(self: &Rc<Self>) {
        if self.celebrated.replace(true) {
            return;
        }
        let animations = gtk::Settings::default().is_none_or(|settings| settings.is_gtk_enable_animations());
        if let (true, Some(season)) = (animations, Season::today()) {
            self.play(season, false);
        }
    }

    fn play(self: &Rc<Self>, season: Season, preview: bool) {
        let seed = glib::real_time() as u64 | 1;
        let previous = self.show.replace(Some(Show {
            season,
            preview,
            start: None,
            last: 0,
            particles: Vec::new(),
            owed: 0.0,
            random: Random(seed),
        }));
        if preview {
            theme::preview(Some(season));
        } else if previous.is_some_and(|show| show.preview) {
            theme::preview(None);
        }
        self.area.set_visible(true);
        if self.ticking.replace(true) {
            return;
        }
        let this = Rc::downgrade(self);
        self.area.add_tick_callback(move |area, clock| {
            let Some(this) = this.upgrade() else { return glib::ControlFlow::Break };
            let going = match this.show.borrow_mut().as_mut() {
                Some(show) => show.step(clock.frame_time(), f64::from(area.width()), f64::from(area.height())),
                None => false,
            };
            area.queue_draw();
            if going {
                return glib::ControlFlow::Continue;
            }
            this.ticking.set(false);
            area.set_visible(false);
            if this.show.take().is_some_and(|show| show.preview) {
                theme::preview(None);
            }
            glib::ControlFlow::Break
        });
    }
}

impl Drop for Effects {
    fn drop(&mut self) {
        // A window closed while a holiday was tried: back to the colors of today.
        if self.show.borrow().as_ref().is_some_and(|show| show.preview) {
            theme::preview(None);
        }
    }
}

/// Plays a holiday's effect, with its colors, over the main window; false
/// without one.
pub fn try_effect(season: Season) -> bool {
    match LAYER.with_borrow(Weak::upgrade) {
        Some(effects) => {
            effects.play(season, true);
            true
        }
        None => false,
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
    fn every_show_ends() {
        for season in Season::ALL {
            let mut show =
                Show { season, preview: false, start: None, last: 0, particles: Vec::new(), owed: 0.0, random: Random(7) };
            let mut frames = 0;
            while show.step(frames * 16_667, 1000.0, 700.0) {
                frames += 1;
                assert!(frames < 60 * 45, "{season:?} never ends");
            }
            assert!(frames > 60 * 14, "{season:?} ended at frame {frames}");
        }
    }
}
