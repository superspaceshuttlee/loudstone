//! Command-line options, parsed once into one value.
//!
//! These used to be a dozen loose fields on `App`, each parsing `std::env::args`
//! again at its own initialiser -- twelve scattered scans, no single place that
//! said what the program accepts, and nothing stopping two of them disagreeing
//! about a flag. Adding a mode meant remembering to touch the struct, the
//! initialiser and the "is this an automated run" list, and forgetting the third
//! is why a new mode would silently stop at the title screen.

/// Everything the program was asked to do on the command line.
#[derive(Clone, Debug, Default)]
pub struct Cli {
    /// `--shot <path>`: render one settled frame to `path` and quit.
    pub shot_path: Option<std::path::PathBuf>,
    /// `--vista`: stand on high ground and look out, for judging terrain.
    pub vista: bool,
    /// `--demo`: carve a hemisphere ahead of the camera before capturing.
    pub demo: bool,
    /// `--models`: line the mobs up on a platform.
    pub models_review: bool,
    /// `--angle <deg>`: which way the reviewed models face.
    pub review_angle: f32,
    /// `--ui <which>`: open a named panel before capturing.
    pub ui_demo: Option<String>,
    /// `--model <which>`: pose one named model before capturing.
    pub model_demo: Option<String>,
    /// `--gauntlet`: let the robot play. Carries the run seed.
    pub gauntlet_seed: Option<u64>,
    /// `--secs <n>`: how long a gauntlet session runs, in simulated seconds.
    pub gauntlet_secs: Option<f32>,
    /// `--title`: show the menu even in a mode that would normally skip it.
    pub force_title: bool,
    /// `--map <path>`: write a top-down map of the surface and quit.
    pub map_path: Option<std::path::PathBuf>,
    /// `--map-span <blocks>`: half-width of the mapped region.
    pub map_span: i32,
}

impl Cli {
    pub fn from_args() -> Self {
        Self {
            shot_path: value_of("--shot").map(std::path::PathBuf::from),
            vista: flag("--vista"),
            demo: flag("--demo"),
            models_review: flag("--models"),
            review_angle: value_of("--angle")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0),
            ui_demo: value_of("--ui"),
            model_demo: value_of("--model"),
            // An unseeded run picks its own seed, and it is printed, so a failure
            // found by chance can still be replayed exactly.
            gauntlet_seed: flag("--gauntlet").then(|| {
                value_of("--seed")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| {
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(1)
                    })
            }),
            gauntlet_secs: value_of("--secs").and_then(|v| v.parse().ok()),
            force_title: flag("--title"),
            map_path: value_of("--map").map(std::path::PathBuf::from),
            map_span: value_of("--map-span")
                .and_then(|v| v.parse().ok())
                .unwrap_or(2048),
        }
    }

    /// True when nobody is at the keyboard, so the title screen is skipped and
    /// simulation begins immediately.
    ///
    /// Derived from the options rather than listed separately: a new mode that
    /// forgot to add itself to a hand-written list would sit at the menu forever
    /// and look like a hang.
    pub fn automated(&self) -> bool {
        !self.force_title
            && (self.shot_path.is_some()
                || self.vista
                || self.demo
                || self.models_review
                || self.ui_demo.is_some()
                || self.model_demo.is_some()
                || self.gauntlet_seed.is_some()
                || self.map_path.is_some())
    }
}

fn flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

fn value_of(name: &str) -> Option<String> {
    std::env::args().skip_while(|a| a != name).nth(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this guards: a mode added to the struct but forgotten in the
    /// "is this automated" list waits at the title screen and reads as a hang.
    #[test]
    fn every_capture_mode_counts_as_automated() {
        let modes: [(&str, fn(&mut Cli)); 8] = [
            ("shot", |c| c.shot_path = Some("x.png".into())),
            ("vista", |c| c.vista = true),
            ("demo", |c| c.demo = true),
            ("models", |c| c.models_review = true),
            ("ui", |c| c.ui_demo = Some("inv".into())),
            ("model", |c| c.model_demo = Some("pig".into())),
            ("gauntlet", |c| c.gauntlet_seed = Some(1)),
            ("map", |c| c.map_path = Some("m.png".into())),
        ];
        for (name, set) in modes {
            let mut cli = Cli::default();
            set(&mut cli);
            assert!(cli.automated(), "--{name} should skip the title screen");
        }
        assert!(!Cli::default().automated(), "a plain run shows the menu");
    }

    #[test]
    fn an_explicit_title_flag_beats_every_capture_mode() {
        let mut cli = Cli::default();
        cli.models_review = true;
        cli.force_title = true;
        assert!(!cli.automated());
    }
}
