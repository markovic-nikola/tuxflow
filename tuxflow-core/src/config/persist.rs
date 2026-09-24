//! Reading and writing the TOML files under `~/.config/tuxflow` and
//! `~/.local/state/tuxflow` — one implementation, so every file gets the
//! same atomic write.
//!
//! The two files under `~/.config` are also written by OTHER machines:
//! people sync that directory (Dropbox, Syncthing), and the app only reads
//! it at launch. Saving the whole in-memory struct would silently revert
//! whatever arrived from elsewhere since then — a project added on the
//! laptop vanishing on both machines the moment the PC toggles a setting.
//! So those files save through [`save_merged`]: only the keys THIS instance
//! changed since it loaded are applied, onto the file as it is now.

use std::fs;
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;
use toml::{Table, Value};

/// Parse `path`, or `None` when it is absent, unreadable or malformed (the
/// last two are logged). `what` names the file in the log.
pub fn read_toml<T: DeserializeOwned>(path: &Path, what: &str) -> Option<T> {
    if !path.exists() {
        return None;
    }
    match fs::read_to_string(path) {
        Ok(content) => match toml::from_str(&content) {
            Ok(value) => {
                log::debug!("Loaded {what} from {}", path.display());
                Some(value)
            }
            Err(e) => {
                log::warn!("Failed to parse {what}: {e}");
                None
            }
        },
        Err(e) => {
            log::warn!("Failed to read {what}: {e}");
            None
        }
    }
}

/// Serialize `value` to `path`, creating the directory. Failures are logged,
/// never raised: a lost save must not take the window down with it.
///
/// Atomic: write a sibling tmp file, then rename over the target. Two
/// writers can race (two TuxFlow versions sharing a file, the background
/// geometry save), and a reader must never see a torn half-write — parsing
/// one as "empty workspace" and saving it back is how a workspace gets wiped.
pub fn write_toml<T: Serialize>(value: &T, path: &Path, what: &str) {
    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        log::error!("Failed to create directory for {what}: {e}");
        return;
    }
    match toml::to_string_pretty(value) {
        Ok(content) => {
            let tmp = path.with_extension("toml.tmp");
            match fs::write(&tmp, content).and_then(|_| fs::rename(&tmp, path)) {
                Ok(()) => log::debug!("Saved {what} to {}", path.display()),
                Err(e) => log::error!("Failed to write {what}: {e}"),
            }
        }
        Err(e) => log::error!("Failed to serialize {what}: {e}"),
    }
}

/// What a synced file looked like to this instance when it loaded it, as
/// the struct serialized — the reference [`save_merged`] diffs against.
/// `None` = never loaded (a `default()` instance): saves write it whole.
#[derive(Debug, Clone, Default)]
pub struct Baseline(Option<Table>);

impl Baseline {
    /// Snapshot `value` as the state the next save diffs against.
    pub fn of<T: Serialize>(value: &T) -> Self {
        Self(to_table(value))
    }
}

fn to_table<T: Serialize>(value: &T) -> Option<Table> {
    match Table::try_from(value) {
        Ok(table) => Some(table),
        Err(e) => {
            log::error!("Failed to serialize for merge: {e}");
            None
        }
    }
}

/// Save `value` to `path` without reverting anyone else's changes: diff it
/// against `baseline`, apply that diff to the file as it is on disk now,
/// write the result, and move the baseline to `value`.
///
/// Tables merge key by key, at any depth. Arrays of plain values (project
/// lists, process order) merge item by item — see [`merge_list`]; other
/// arrays, i.e. a project's command list, are replaced whole when this
/// instance changed them (an item-wise merge there would keep both
/// machines' edits of one command as two commands of the same name).
/// Scalars: ours wins where ours changed. Keys this build does not know
/// (written by a newer version) survive, and the file keeps its layout
/// (toml's `preserve_order`). `retired` names top-level keys moved elsewhere
/// (see `config::state`): the diff never touches them, so they are dropped
/// explicitly.
pub fn save_merged<T: Serialize>(
    value: &T,
    baseline: &mut Baseline,
    path: &Path,
    what: &str,
    retired: &[&str],
) {
    let Some(ours) = to_table(value) else { return };
    let mut merged = match &baseline.0 {
        Some(base) => {
            let mut disk: Table = read_toml(path, what).unwrap_or_default();
            apply_changes(base, &ours, &mut disk);
            disk
        }
        None => ours.clone(),
    };
    for key in retired {
        merged.remove(*key);
    }
    write_toml(&merged, path, what);
    baseline.0 = Some(ours);
}

/// Apply to `target` every difference between `base` and `ours`.
fn apply_changes(base: &Table, ours: &Table, target: &mut Table) {
    for (key, value) in ours {
        match (base.get(key), value) {
            (Some(old), new) if old == new => {}
            (Some(Value::Array(old)), Value::Array(new)) if all_plain(old) && all_plain(new) => {
                let merged = match target.get(key) {
                    Some(Value::Array(theirs)) if all_plain(theirs) => merge_list(old, new, theirs),
                    _ => new.clone(),
                };
                target.insert(key.clone(), Value::Array(merged));
            }
            (Some(Value::Table(old)), Value::Table(new)) => {
                let entry = target
                    .entry(key.clone())
                    .or_insert_with(|| Value::Table(Table::new()));
                match entry {
                    Value::Table(t) => apply_changes(old, new, t),
                    other => *other = value.clone(),
                }
            }
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
    for key in base.keys() {
        if !ours.contains_key(key) {
            target.remove(key);
        }
    }
}

fn all_plain(items: &[Value]) -> bool {
    items
        .iter()
        .all(|v| !matches!(v, Value::Table(_) | Value::Array(_)))
}

/// Three-way merge of a list whose items are identities (paths, names):
/// OUR order, minus what the file lost since `base` (removed elsewhere),
/// then what the file gained that we never saw (added elsewhere). What we
/// removed stays removed; a reorder here keeps its order.
fn merge_list(base: &[Value], ours: &[Value], theirs: &[Value]) -> Vec<Value> {
    let removed_elsewhere = |v: &&Value| base.contains(v) && !theirs.contains(v);
    let added_elsewhere = |v: &&Value| !base.contains(v) && !ours.contains(v);
    ours.iter()
        .filter(|v| !removed_elsewhere(v))
        .chain(theirs.iter().filter(added_elsewhere))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(src: &str) -> Table {
        toml::from_str(src).expect("fixture")
    }

    fn merged(base: &str, ours: &str, disk: &str) -> Table {
        let mut disk = table(disk);
        apply_changes(&table(base), &table(ours), &mut disk);
        disk
    }

    /// The whole point: a key the other machine changed survives a save
    /// here that changed something else.
    #[test]
    fn only_our_changes_reach_the_file() {
        let out = merged(
            "[a]\nfont = 12\ntheme = 'dark'",
            "[a]\nfont = 15\ntheme = 'dark'",
            "[a]\nfont = 12\ntheme = 'light'",
        );
        assert_eq!(out, table("[a]\nfont = 15\ntheme = 'light'"));
    }

    /// Map entries are keys too: a project added elsewhere stays when we
    /// add another, and one we removed goes without touching theirs.
    #[test]
    fn map_entries_merge_by_key() {
        let out = merged(
            "[names]\n'/a' = 'A'\n'/b' = 'B'",
            "[names]\n'/a' = 'A'\n'/c' = 'C'",
            "[names]\n'/a' = 'A'\n'/b' = 'B'\n'/d' = 'D'",
        );
        assert_eq!(out, table("[names]\n'/a' = 'A'\n'/c' = 'C'\n'/d' = 'D'"));
    }

    /// Both machines added a project: both stay. Found replaying the real
    /// projects.toml, where replacing the list whole lost the laptop's.
    #[test]
    fn lists_keep_additions_from_both_sides() {
        let theirs = "dirs = ['/a', '/b', '/laptop']";
        assert_eq!(
            merged("dirs = ['/a', '/b']", "dirs = ['/a', '/b']", theirs),
            table(theirs)
        );
        assert_eq!(
            merged("dirs = ['/a', '/b']", "dirs = ['/a', '/b', '/pc']", theirs),
            table("dirs = ['/a', '/b', '/pc', '/laptop']")
        );
    }

    /// A removal on either side sticks, and our reorder keeps our order.
    #[test]
    fn lists_keep_removals_and_our_order() {
        assert_eq!(
            merged(
                "dirs = ['/a', '/b', '/c']",
                "dirs = ['/c', '/a', '/b']",
                "dirs = ['/a', '/c']"
            ),
            table("dirs = ['/c', '/a']")
        );
        assert_eq!(
            merged(
                "dirs = ['/a', '/b']",
                "dirs = ['/b']",
                "dirs = ['/a', '/b', '/x']"
            ),
            table("dirs = ['/b', '/x']")
        );
    }

    /// A command list is replaced whole: both machines editing `dev` must
    /// not come out as two `dev` commands.
    #[test]
    fn lists_of_tables_are_replaced_whole() {
        let base = "[[cmd]]\nname = 'dev'\ncommand = 'a'";
        let ours = "[[cmd]]\nname = 'dev'\ncommand = 'b'";
        let theirs = "[[cmd]]\nname = 'dev'\ncommand = 'c'";
        assert_eq!(merged(base, ours, theirs), table(ours));
    }

    /// With `preserve_order`, a merged save keeps the file's own layout.
    #[test]
    fn a_merge_keeps_the_file_layout() {
        let out = merged("z = 1\na = 1", "z = 2\na = 1", "z = 1\na = 1");
        assert_eq!(toml::to_string(&out).expect("serialise"), "z = 2\na = 1\n");
    }

    #[test]
    fn save_merged_drops_retired_keys_and_moves_the_baseline() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("f.toml");
        std::fs::write(&file, "keep = 1\nnew = 1\n[window]\nwidth = 9\n").expect("fixture");

        let mut base = Baseline::of(&table("keep = 1\nnew = 1"));
        save_merged(
            &table("keep = 1\nnew = 2"),
            &mut base,
            &file,
            "t",
            &["window"],
        );
        std::fs::write(&file, "keep = 5\nnew = 2\n").expect("the other machine");
        // Unchanged since the last save, so the other machine's 5 stays.
        save_merged(
            &table("keep = 1\nnew = 2"),
            &mut base,
            &file,
            "t",
            &["window"],
        );

        let on_disk: Table = read_toml(&file, "t").expect("written");
        assert_eq!(on_disk, table("keep = 5\nnew = 2"));
    }
}
