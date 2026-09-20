//! The feature this port adds: extra scores supplied at the command line.

use std::path::PathBuf;

use thonkr::score::{apply_set, built_in, Catalog, Origin, ScoreSpec};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("thonkr-scores-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    path
}

fn write(name: &str, text: &str) -> PathBuf {
    let path = scratch(name);
    std::fs::write(&path, text).unwrap();
    path
}

/// A score under a name nothing ships, so loading it adds rather than replaces.
const CUSTOM: &str = r#"
[scores.custom]
description = "almost nothing, very slowly"
duration = 3600
spread = 0.6
voices = 3
density   = { range = [0.2, 4.0],   seg = [30, 180], rand = [0.1, 0.2] }
length    = { range = [0.06, 0.10], seg = [30, 180], rand = [0.1, 0.2] }
attack    = { range = [0.35, 0.50], seg = [30, 180], rand = [0.1, 0.1] }
position  = { range = [0.0, 1.0],   seg = [20, 120], rand = [0.1, 0.05] }
transpose = { range = [-24.0, 0.0], seg = [30, 180], rand = [0.1, 0.08] }
balance   = { range = [0.0, 1.0],   seg = [10, 60],  rand = [0.3, 0.3] }
"#;

// --------------------------------------------------------------------------
// score files
// --------------------------------------------------------------------------

#[test]
fn a_score_file_adds_a_score_that_is_not_already_there() {
    let path = write("custom.toml", CUSTOM);
    let catalog = Catalog::load(std::slice::from_ref(&path)).unwrap();

    let base = Catalog::load(&[]).unwrap().names().len();
    assert_eq!(
        catalog.names().len(),
        base + 1,
        "the scores already there, plus one"
    );
    let score = catalog.get("custom").expect("custom was loaded");
    assert_eq!(score.description, "almost nothing, very slowly");
    assert_eq!(score.duration, 3600.0);
    assert_eq!(score.voices, 3);
    assert_eq!(score.density.range, [0.2, 4.0]);
    assert_eq!(catalog.origin("custom"), Some(&Origin::File(path)));
    assert!(score.problems().is_empty(), "{:?}", score.problems());
}

#[test]
fn a_score_file_can_be_four_lines_because_the_rest_defaults() {
    let path = write(
        "small.toml",
        "[scores.slow]\ndensity = { range = [0.5, 20.0] }\n",
    );
    let catalog = Catalog::load(&[path]).unwrap();
    let score = catalog.get("slow").unwrap();
    let flowing = built_in("flowing").unwrap();

    assert_eq!(score.density.range, [0.5, 20.0], "the field that was given");
    assert_eq!(
        score.density.seg, flowing.density.seg,
        "the fields that were not"
    );
    assert_eq!(score.density.rand, flowing.density.rand);
    assert_eq!(score.length, flowing.length, "untouched parameters");
    assert_eq!(score.duration, flowing.duration);
    assert_eq!(score.voices, flowing.voices);
}

#[test]
fn a_file_can_replace_a_built_in_and_keeps_its_place_in_the_listing() {
    let path = write("override.toml", "[scores.hectic]\nduration = 60\n");
    let catalog = Catalog::load(std::slice::from_ref(&path)).unwrap();

    let before = Catalog::load(&[]).unwrap();
    assert_eq!(
        catalog.names(),
        before.names(),
        "replacing a score must not add a name or move one"
    );
    let hectic = catalog.get("hectic").unwrap();
    assert_eq!(hectic.duration, 60.0, "the field that was overridden");
    assert_eq!(
        hectic.density,
        built_in("hectic").unwrap().density,
        "an override builds on the score it replaces, not on flowing"
    );
    assert_eq!(catalog.origin("hectic"), Some(&Origin::File(path)));
}

#[test]
fn the_last_file_named_wins() {
    let a = write("a.toml", "[scores.mine]\nduration = 10\nspread = 0.1\n");
    let b = write("b.toml", "[scores.mine]\nduration = 20\n");
    let catalog = Catalog::load(&[a, b.clone()]).unwrap();
    let score = catalog.get("mine").unwrap();

    assert_eq!(score.duration, 20.0, "the later file wins");
    assert_eq!(score.spread, 0.1, "and builds on the earlier one");
    assert_eq!(catalog.origin("mine"), Some(&Origin::File(b)));
}

/// The scores shipped in the repository must all work. They go out in the
/// release archive, so a broken one is a broken release.
#[test]
fn every_shipped_score_loads_and_is_usable() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scores");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    assert!(files.len() >= 4, "only {} score files ship", files.len());

    for path in &files {
        let catalog = Catalog::load(std::slice::from_ref(path))
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let added: Vec<String> = catalog
            .iter()
            .filter(|(_, _, origin)| matches!(origin, Origin::File(_)))
            .map(|(name, _, _)| name.to_string())
            .collect();
        assert!(!added.is_empty(), "{} defines no score", path.display());

        for name in added {
            let score = catalog.get(&name).unwrap();
            assert!(
                score.problems().is_empty(),
                "{} score {name}: {:?}",
                path.display(),
                score.problems()
            );
            assert!(
                !score.description.is_empty(),
                "{} score {name} has no description",
                path.display()
            );
        }
    }

    // Loading them all at once must work too, which catches a name collision.
    let catalog = Catalog::load(&files).unwrap();
    let names = catalog.names();
    let distinct: std::collections::BTreeSet<_> = names.iter().collect();
    assert_eq!(
        names.len(),
        distinct.len(),
        "two scores share a name: {names:?}"
    );
}

/// The point of embedding: a binary on its own, with no files beside it,
/// carries every score it lists.
#[test]
fn the_shipped_scores_are_inside_the_binary() {
    let catalog = Catalog::load(&[]).unwrap();

    for name in ["glacial", "shimmer", "rumble"] {
        let score = catalog
            .get(name)
            .unwrap_or_else(|| panic!("{name} is not embedded; check build.rs"));
        assert!(
            score.problems().is_empty(),
            "{name}: {:?}",
            score.problems()
        );
        assert!(!score.description.is_empty(), "{name} has no description");
        assert_eq!(
            catalog.origin(name),
            Some(&Origin::BuiltIn),
            "an embedded score should look built in, not like a file"
        );
    }

    // The worked example teaches the file format. It is not a score to ship,
    // so build.rs leaves it out.
    assert!(
        catalog.get("example").is_none(),
        "example.toml should not be embedded"
    );

    assert_eq!(catalog.names().len(), 8, "five written plus three embedded");
}

/// An embedded score is still just a score, so a file can replace it.
#[test]
fn a_file_can_override_an_embedded_score() {
    let path = write("override_rumble.toml", "[scores.rumble]\nduration = 42\n");
    let catalog = Catalog::load(std::slice::from_ref(&path)).unwrap();
    let rumble = catalog.get("rumble").unwrap();

    assert_eq!(rumble.duration, 42.0, "the field that was overridden");
    assert_eq!(
        rumble.transpose,
        Catalog::load(&[]).unwrap().get("rumble").unwrap().transpose,
        "an override builds on the embedded score, not on flowing"
    );
    assert_eq!(catalog.origin("rumble"), Some(&Origin::File(path)));
    assert_eq!(catalog.names().len(), 8, "an override adds no new name");
}

#[test]
fn a_score_file_that_is_not_there_is_an_error() {
    let missing = scratch("no-such-file.toml");
    let err = Catalog::load(&[missing]).unwrap_err().to_string();
    assert!(err.contains("no-such-file.toml"), "{err}");
}

#[test]
fn a_file_that_is_not_valid_toml_says_so() {
    let path = write("broken.toml", "[scores.bad\nduration = ");
    let err = Catalog::load(&[path]).unwrap_err().to_string();
    assert!(err.contains("broken.toml"), "{err}");
}

#[test]
fn a_misspelled_field_is_caught_rather_than_ignored() {
    let path = write("typo.toml", "[scores.mine]\ndurations = 60\n");
    let err = Catalog::load(&[path]).unwrap_err().to_string();
    assert!(err.contains("durations"), "the typo must be named: {err}");
}

#[test]
fn a_file_with_no_scores_in_it_says_so() {
    let path = write("empty.toml", "duration = 60\n");
    let err = Catalog::load(&[path]).unwrap_err().to_string();
    assert!(err.contains("[scores.NAME]"), "{err}");
}

#[test]
fn an_unknown_score_lists_what_there_is() {
    let path = write("custom2.toml", CUSTOM);
    let catalog = Catalog::load(&[path]).unwrap();
    let err = catalog.require("galcial").unwrap_err().to_string();
    assert!(err.contains("unknown score: galcial"), "{err}");
    assert!(
        err.contains("glacial"),
        "the real name must be listed: {err}"
    );
    assert!(err.contains("flowing"), "{err}");
}

// --------------------------------------------------------------------------
// --set
// --------------------------------------------------------------------------

fn set(arg: &str) -> Result<ScoreSpec, String> {
    let mut score = built_in("flowing").unwrap();
    apply_set(&mut score, arg).map_err(|e| e.to_string())?;
    Ok(score)
}

#[test]
fn set_changes_one_field_of_a_parameter() {
    let score = set("density.range=1,400").unwrap();
    let flowing = built_in("flowing").unwrap();
    assert_eq!(score.density.range, [1.0, 400.0]);
    assert_eq!(score.density.seg, flowing.density.seg, "nothing else moves");
    assert_eq!(score.length, flowing.length);

    assert_eq!(set("length.seg=2,9").unwrap().length.seg, [2.0, 9.0]);
    assert_eq!(set("balance.rand=3,0.4").unwrap().balance.rand, [3.0, 0.4]);
    assert_eq!(
        set("transpose.range=-24,0").unwrap().transpose.range,
        [-24.0, 0.0]
    );
}

#[test]
fn set_changes_the_whole_score_fields() {
    assert_eq!(set("spread=0.6").unwrap().spread, 0.6);
    assert_eq!(set("duration=90").unwrap().duration, 90.0);
    assert_eq!(set("voices=3").unwrap().voices, 3);
    assert!(set("stretch=true").unwrap().stretch);
    assert!(!set("stretch=false").unwrap().stretch);
}

#[test]
fn several_sets_apply_in_the_order_they_are_given() {
    let mut score = built_in("flowing").unwrap();
    for arg in ["density.range=1,400", "spread=0.6", "density.range=2,800"] {
        apply_set(&mut score, arg).unwrap();
    }
    assert_eq!(score.density.range, [2.0, 800.0], "the last one wins");
    assert_eq!(score.spread, 0.6);
}

#[test]
fn spaces_around_a_set_are_forgiven() {
    assert_eq!(
        set(" density.range = 1 , 400 ").unwrap().density.range,
        [1.0, 400.0]
    );
}

#[test]
fn a_bad_set_key_lists_the_keys_that_work() {
    for arg in ["densty.range=1,2", "density.rnge=1,2", "sprad=0.5"] {
        let err = set(arg).unwrap_err();
        assert!(err.contains("no such key"), "{arg}: {err}");
        assert!(
            err.contains("density.range"),
            "{arg} must list the keys: {err}"
        );
        assert!(err.contains("spread"), "{arg} must list the keys: {err}");
    }
}

#[test]
fn a_bad_set_value_names_the_key_and_the_shape() {
    let err = set("density.range=1").unwrap_err();
    assert!(err.contains("density.range"), "{err}");
    assert!(err.contains("two numbers"), "{err}");

    let err = set("density.range=one,two").unwrap_err();
    assert!(err.contains("two numbers"), "{err}");

    let err = set("spread=loud").unwrap_err();
    assert!(err.contains("spread takes one number"), "{err}");

    let err = set("voices=2.5").unwrap_err();
    assert!(err.contains("whole number"), "{err}");

    let err = set("stretch=yes").unwrap_err();
    assert!(err.contains("true or false"), "{err}");
}

#[test]
fn a_set_with_no_equals_sign_says_what_the_shape_is() {
    let err = set("density.range").unwrap_err();
    assert!(err.contains("KEY=VALUE"), "{err}");
}

// --------------------------------------------------------------------------
// validation
// --------------------------------------------------------------------------

fn problems_of(args: &[&str]) -> Vec<String> {
    let mut score = built_in("flowing").unwrap();
    for arg in args {
        apply_set(&mut score, arg).unwrap();
    }
    score.problems()
}

#[test]
fn every_built_in_passes_validation() {
    for name in thonkr::score::BUILT_IN_NAMES {
        let score = built_in(name).unwrap();
        assert!(
            score.problems().is_empty(),
            "{name}: {:?}",
            score.problems()
        );
        score.validate(name).unwrap();
    }
}

#[test]
fn each_rule_catches_its_own_mistake() {
    let cases: [(&str, &str); 12] = [
        (
            "density.range=400,1",
            "low bound 400 is above the high bound",
        ),
        ("density.range=0,100", "low bound must be greater than 0"),
        ("length.range=0,0.1", "low bound must be greater than 0"),
        ("length.range=0.1,2", "above the limit of 1 s"),
        ("attack.range=0.1,0.9", "outside 0 to 0.5"),
        ("position.range=0,2", "outside 0 to 1"),
        ("balance.range=-1,1", "outside 0 to 1"),
        ("density.seg=0,10", "both times must be greater than 0"),
        ("density.seg=60,8", "is longer than the longest gap"),
        ("density.rand=5000,0.1", "above the limit of 1000 Hz"),
        ("spread=1.5", "outside 0 to 1"),
        ("voices=0", "outside 1 to 16"),
    ];
    for (arg, wanted) in cases {
        let problems = problems_of(&[arg]);
        assert!(
            problems.iter().any(|p| p.contains(wanted)),
            "{arg} should have been caught, got {problems:?}"
        );
    }
    assert!(problems_of(&["duration=-5"])
        .iter()
        .any(|p| p.contains("duration")));
}

#[test]
fn every_problem_is_reported_at_once() {
    let problems = problems_of(&["spread=1.5", "voices=99", "attack.range=0.1,0.9"]);
    assert!(problems.len() >= 3, "got only {problems:?}");
    assert!(problems.iter().any(|p| p.contains("spread")));
    assert!(problems.iter().any(|p| p.contains("voices")));
    assert!(problems.iter().any(|p| p.contains("attack")));

    let mut score = built_in("flowing").unwrap();
    score.spread = 1.5;
    score.voices = 99;
    let err = score.validate("mine").unwrap_err().to_string();
    assert!(err.contains("score \"mine\" is not usable"), "{err}");
    assert!(err.contains("spread") && err.contains("voices"), "{err}");
}

#[test]
fn a_rand_rate_or_depth_of_zero_is_allowed_and_turns_the_randomizer_off() {
    assert!(problems_of(&["density.rand=0,0"]).is_empty());
    assert!(problems_of(&["density.rand=0,0.5"]).is_empty());
    assert!(!problems_of(&["density.rand=-1,0.5"]).is_empty());
}

// --------------------------------------------------------------------------
// --dump-score
// --------------------------------------------------------------------------

#[test]
fn a_dumped_score_loads_back_as_the_same_score() {
    for name in thonkr::score::BUILT_IN_NAMES {
        let score = built_in(name).unwrap();
        let text = score.to_toml(name);
        assert!(text.starts_with(&format!("[scores.{name}]")), "{text}");

        let path = write(&format!("round_{name}.toml"), &text);
        let catalog = Catalog::load(&[path]).unwrap();
        assert_eq!(
            catalog.get(name),
            Some(&score),
            "{name} changed on the way round"
        );
    }
}

#[test]
fn a_dumped_score_is_a_usable_starting_point_for_editing() {
    let mut score = built_in("sparse").unwrap();
    apply_set(&mut score, "density.range=0.1,3").unwrap();
    score.description = "my own \"quiet\" one".to_string();

    let path = write("edited.toml", &score.to_toml("mine"));
    let catalog = Catalog::load(&[path]).unwrap();
    let back = catalog.get("mine").unwrap();
    assert_eq!(back.density.range, [0.1, 3.0]);
    assert_eq!(
        back.description, "my own \"quiet\" one",
        "quotes must survive"
    );
    assert!(back.problems().is_empty());
}
