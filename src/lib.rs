//! The thOnk_0+2 granular engine, reimplemented as a command line renderer.
//!
//! thOnk_0+2 (Arjen van der Schoot / Audio Ease, 1996-1999) turned a short mono
//! AIFF file into hours of granular sound over which the user had, by design,
//! no control at all. This crate rebuilds the synthesis model from the
//! description in that manual. It carries no code from the original.
//!
//! See `RUST_MIGRATION.md` for the port plan and the phase each module belongs
//! to.

pub mod audio;
pub mod curve;
pub mod engine;
pub mod error;
pub mod score;
pub mod session;

pub use error::{Error, Result};

/// Format a count of seconds as `h:mm:ss`.
pub fn hms(seconds: f64) -> String {
    let s = if seconds.is_finite() && seconds > 0.0 {
        seconds as u64
    } else {
        0
    };
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::hms;

    #[test]
    fn hms_matches_the_python_format() {
        assert_eq!(hms(0.0), "0:00:00");
        assert_eq!(hms(59.9), "0:00:59");
        assert_eq!(hms(60.0), "0:01:00");
        assert_eq!(hms(1200.0), "0:20:00");
        assert_eq!(hms(3600.0), "1:00:00");
        assert_eq!(hms(3661.0), "1:01:01");
    }

    #[test]
    fn hms_survives_nonsense_input() {
        assert_eq!(hms(-5.0), "0:00:00");
        assert_eq!(hms(f64::NAN), "0:00:00");
    }
}
