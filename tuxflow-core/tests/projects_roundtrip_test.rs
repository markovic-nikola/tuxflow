use tuxflow_core::config::projects::SavedProjects;

/// Serialising the same state twice must produce byte-identical TOML.
///
/// The maps in `SavedProjects` were `HashMap`, whose iteration order is
/// randomised per process, so every save rewrote the whole file in a new
/// order — turning a one-key change into a whole-file diff for anyone
/// tracking `projects.toml` in git. `BTreeMap` sorts by key, so output is
/// stable across saves and across machines.
#[test]
fn serialization_is_deterministic() {
    let toml_src = r#"
directories = ["/b/two", "/a/one"]

[icons]
"/b/two" = "b.svg"
"/a/one" = "a.svg"

[names]
"/b/two" = "Two"
"/a/one" = "One"

[last_used]
"/b/two" = 200
"/a/one" = 100

[[custom_commands."/a/one"]]
name = "dev"
command = "npm run dev"
working_dir = "/a/one"

[custom_commands."/a/one".env]
ZED = "1"
ALPHA = "2"
"#;

    let parsed: SavedProjects = toml::from_str(toml_src).expect("fixture must parse");
    let first = toml::to_string_pretty(&parsed).expect("must serialise");

    // Re-parse and re-serialise: a full round trip must be a fixed point.
    let reparsed: SavedProjects = toml::from_str(&first).expect("output must re-parse");
    let second = toml::to_string_pretty(&reparsed).expect("must serialise");
    assert_eq!(first, second, "round trip must be byte-identical");

    // And the order must be sorted, not insertion order.
    let icon_a = first.find(r#""/a/one" = "a.svg""#).expect("a icon present");
    let icon_b = first.find(r#""/b/two" = "b.svg""#).expect("b icon present");
    assert!(icon_a < icon_b, "icon keys must be sorted");

    let env_alpha = first.find("ALPHA").expect("ALPHA present");
    let env_zed = first.find("ZED").expect("ZED present");
    assert!(env_alpha < env_zed, "env keys must be sorted");

    // `directories` is a Vec — user-defined order, must survive untouched.
    let dir_b = first.find("/b/two").expect("directories entry present");
    let dir_a = first.find("/a/one").expect("directories entry present");
    assert!(dir_b < dir_a, "directories must keep their declared order");
}
