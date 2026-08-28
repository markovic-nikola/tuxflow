//! Git state for the status bar and the Git Changes view, local or over
//! ssh. The chip's one `status --porcelain=v2 --branch` round trip yields
//! branch, ahead/behind and the changed-entry count; the rest of the
//! module is the plumbing the changes view runs on — the file list, the
//! per-file diff with its syntect highlighting, and the write actions
//! (commit / push / pull / sync).
//!
//! Everything here is BLOCKING (a process spawn locally, an ssh round
//! trip remotely) — call it from a worker thread, never from a UI loop.
//! Both shells consume this: the highlighting is emitted as plain
//! `(line, offset, len, hex)` tuples so neither GTK's TextTag table nor
//! iced's text spans is baked into the answer.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::LazyLock;

use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

use crate::remote::{ProjectLocation, sh_quote, ssh_mux_options};

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_nonewlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatus {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    /// Working-tree entries (changed / untracked / conflicted).
    pub changed: u32,
}

/// A git invocation in the project's real location: plain `git` locally,
/// `ssh host 'cd dir && git …'` (mux, BatchMode) for remote projects.
pub fn git_command(location: &ProjectLocation, args: &[&str]) -> std::process::Command {
    match location {
        ProjectLocation::Local(dir) => {
            let mut cmd = std::process::Command::new("git");
            cmd.args(args).current_dir(dir);
            cmd
        }
        ProjectLocation::Ssh { host, dir } => {
            let mut cmd = std::process::Command::new("ssh");
            cmd.args(ssh_mux_options());
            cmd.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]);
            cmd.arg(host);
            let git_args = args
                .iter()
                .map(|a| sh_quote(a))
                .collect::<Vec<_>>()
                .join(" ");
            cmd.arg(format!("cd {} && git {}", sh_quote(dir), git_args));
            cmd
        }
    }
}

/// None = not a git repo, git absent, or (remote) host unreachable —
/// the chip simply doesn't show.
pub fn query_status(location: &ProjectLocation) -> Option<GitStatus> {
    let output = git_command(location, &["status", "--porcelain=v2", "--branch"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_porcelain_v2(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_porcelain_v2(text: &str) -> GitStatus {
    let mut status = GitStatus {
        branch: String::new(),
        ahead: 0,
        behind: 0,
        changed: 0,
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            status.branch = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            for part in rest.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    status.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    status.behind = n.parse().unwrap_or(0);
                }
            }
        } else if !line.starts_with('#') && !line.is_empty() {
            status.changed += 1;
        }
    }
    status
}

/// Run a git command and split the answer the way a UI wants it: stdout
/// on success, the trimmed stderr as the error message on failure (that
/// string goes straight into an error dialog).
pub fn run_git_command(location: &ProjectLocation, args: &[&str]) -> Result<String, String> {
    let output = git_command(location, args)
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Stdout of a git command, or empty on any failure — for the counters,
/// where "couldn't ask" and "the answer is zero" lead to the same chip.
fn git_output(location: &ProjectLocation, args: &[&str]) -> String {
    match git_command(location, args).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => String::new(),
    }
}

pub fn commits_ahead(location: &ProjectLocation) -> usize {
    git_output(location, &["rev-list", "--count", "@{u}..HEAD"])
        .trim()
        .parse()
        .unwrap_or(0)
}

pub fn commits_behind(location: &ProjectLocation) -> usize {
    git_output(location, &["rev-list", "--count", "HEAD..@{u}"])
        .trim()
        .parse()
        .unwrap_or(0)
}

pub fn dirty_file_count(location: &ProjectLocation) -> usize {
    git_output(location, &["status", "--porcelain=v1"])
        .lines()
        .filter(|l| !l.is_empty())
        .count()
}

/// Working-tree line stats vs HEAD: (files_changed, added, removed).
/// Includes staged changes; untracked files are NOT counted (git diff
/// doesn't see them) — track those separately via `untracked_count`.
pub fn diff_shortstat(location: &ProjectLocation) -> (usize, usize, usize) {
    parse_shortstat(&git_output(location, &["diff", "HEAD", "--shortstat"]))
}

/// " 3 files changed, 120 insertions(+), 45 deletions(-)" — each clause
/// leads with its number, so the label word after it says which counter
/// it is. Clauses git omits (a diff with no deletions has no deletion
/// clause) simply stay zero.
fn parse_shortstat(text: &str) -> (usize, usize, usize) {
    let (mut files, mut added, mut removed) = (0, 0, 0);
    for part in text.split(',') {
        let num: usize = part
            .trim()
            .split(' ')
            .next()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        if part.contains("file") {
            files = num;
        } else if part.contains("insertion") {
            added = num;
        } else if part.contains("deletion") {
            removed = num;
        }
    }
    (files, added, removed)
}

pub fn untracked_count(location: &ProjectLocation) -> usize {
    git_output(location, &["status", "--porcelain=v1"])
        .lines()
        .filter(|l| l.starts_with("??"))
        .count()
}

/// What the status bar's changes chip shows: line counts vs HEAD plus the
/// untracked file count `git diff` can't see.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffStat {
    pub files: usize,
    pub added: usize,
    pub removed: usize,
    pub untracked: usize,
}

/// Both halves of the chip in one call — two round trips, so worker-thread
/// only, but the caller gets a single value to hand the UI.
pub fn query_diffstat(location: &ProjectLocation) -> DiffStat {
    let (files, added, removed) = diff_shortstat(location);
    DiffStat {
        files,
        added,
        removed,
        untracked: untracked_count(location),
    }
}

pub fn fetch(location: &ProjectLocation) {
    let _ = git_command(location, &["fetch"]).output();
}

pub fn current_branch(location: &ProjectLocation) -> Option<String> {
    let name = run_git_command(location, &["branch", "--show-current"])
        .ok()?
        .trim()
        .to_string();
    if name.is_empty() { None } else { Some(name) }
}

/// Whether the project root contains a `.git`. Local is a cheap stat;
/// remote is an ssh round trip — call off the main thread for remote.
pub fn has_git_repo(location: &ProjectLocation) -> bool {
    match location {
        ProjectLocation::Local(dir) => dir.join(".git").exists(),
        ProjectLocation::Ssh { host, dir } => {
            crate::remote::fs::remote_dir_exists(host, &format!("{}/.git", dir)).unwrap_or(false)
        }
    }
}

/// One-click sync: fetch, fast-forward pull if behind, push if ahead.
/// `--ff-only` keeps it safe — diverged histories error out instead of
/// merging, and the caller should point the user at the Git Changes
/// view. Blocking (several network round trips) — call on a worker.
pub fn sync_with_remote(location: &ProjectLocation) -> Result<(), String> {
    run_git_command(location, &["fetch"])?;
    if commits_behind(location) > 0 {
        run_git_command(location, &["pull", "--ff-only"])?;
    }
    // commits_ahead needs an upstream to compare against, so a plain
    // `git push` is always enough when it's > 0.
    if commits_ahead(location) > 0 {
        run_git_command(location, &["push"])?;
    }
    Ok(())
}

/// Push, teaching git the upstream the first time a branch is pushed.
/// A brand-new branch has no `@{u}`, so `git push` refuses with an
/// explanation instead of guessing — the retry supplies what it asked for.
pub fn push(location: &ProjectLocation) -> Result<(), String> {
    match run_git_command(location, &["push"]) {
        Ok(_) => Ok(()),
        Err(e) if e.contains("no upstream") || e.contains("set-upstream") => {
            let branch = current_branch(location).ok_or(e)?;
            run_git_command(location, &["push", "-u", "origin", &branch]).map(|_| ())
        }
        Err(e) => Err(e),
    }
}

/// Fast-forward pull. The retry covers a transient the initial pull after
/// a fresh clone/fetch hits — "no such ref was fetched" or "Cannot
/// fast-forward to multiple branches" when the prior fetch left the
/// upstream config in an intermediate state; an explicit fetch clears it.
pub fn pull(location: &ProjectLocation) -> Result<(), String> {
    run_git_command(location, &["pull", "--ff-only"])
        .or_else(|e| {
            if e.contains("no such ref was fetched")
                || e.contains("Cannot fast-forward to multiple branches")
            {
                run_git_command(location, &["fetch"])
                    .and_then(|_| run_git_command(location, &["pull", "--ff-only"]))
            } else {
                Err(e)
            }
        })
        .map(|_| ())
}

/// Stage everything and commit. `add -A` matches what the changes view
/// shows: the list is the whole working tree, staged or not, so a commit
/// from it takes the whole working tree.
pub fn commit_all(location: &ProjectLocation, message: &str) -> Result<(), String> {
    run_git_command(location, &["add", "-A"])?;
    run_git_command(location, &["commit", "-m", message]).map(|_| ())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
}

impl FileStatus {
    /// Single-letter badge, as git itself abbreviates them.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "U",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub status: FileStatus,
    /// Whether the change is in the index — decides which `git diff`
    /// form shows it.
    pub staged: bool,
}

/// One `git status --porcelain=v1` row: two status columns (index,
/// worktree) then the path from column 4.
fn parse_status_line(line: &str) -> Option<ChangedFile> {
    if line.len() < 4 {
        return None;
    }
    let bytes = line.as_bytes();
    let index = bytes[0];
    let worktree = bytes[1];
    let path = line[3..].to_string();

    let (status, staged) = match (index, worktree) {
        (b'?', b'?') => (FileStatus::Untracked, false),
        (b'A', _) => (FileStatus::Added, true),
        (b'D', _) => (FileStatus::Deleted, true),
        (b'R', _) => (FileStatus::Renamed, true),
        (b'M', _) => (FileStatus::Modified, true),
        (_, b'M') => (FileStatus::Modified, false),
        (_, b'D') => (FileStatus::Deleted, false),
        _ => return None,
    };

    Some(ChangedFile {
        path,
        status,
        staged,
    })
}

pub fn changed_files(location: &ProjectLocation) -> Vec<ChangedFile> {
    git_output(location, &["status", "--porcelain=v1"])
        .lines()
        .filter_map(parse_status_line)
        .collect()
}

/// Cheap "did anything change?" probe for the changes view's poll —
/// hashing the porcelain output is one round trip and tells us whether
/// the (much more expensive) file list and diff need rebuilding.
pub fn status_hash(location: &ProjectLocation) -> u64 {
    let mut hasher = DefaultHasher::new();
    git_output(location, &["status", "--porcelain=v1"]).hash(&mut hasher);
    hasher.finish()
}

/// A file's diff, plus syntax colours as `(line_index, byte_offset_in_line,
/// length, hex_color)`. Deliberately not a widget tree: GTK applies these
/// as TextTags, iced as text spans.
#[derive(Debug, Clone, Default)]
pub struct DiffResult {
    pub text: String,
    pub highlights: Vec<(usize, usize, usize, String)>,
    /// The parts of a `-`/`+` pair that actually differ, as
    /// `(line_index, byte_offset_in_line, length)`. Same indexing
    /// convention as `highlights` — offsets include git's leading marker,
    /// so both lists slice into the same line the caller holds, and a
    /// shell can merge them without knowing how either was produced.
    pub word_ranges: Vec<(usize, usize, usize)>,
}

/// Cap diff text before highlighting. Minified files produce one giant
/// line that blows past the line guard, then locks whichever thread has
/// to walk the resulting tokens. The byte cap is the real guard; the line
/// cap handles ordinary huge diffs.
const MAX_DIFF_BYTES: usize = 256 * 1024;
const MAX_DIFF_LINES: usize = 5000;

pub fn load_diff(location: &ProjectLocation, file: &ChangedFile) -> DiffResult {
    // An untracked file has no blob to diff against, so it is diffed
    // against /dev/null to render as all-additions.
    let args: Vec<&str> = if matches!(file.status, FileStatus::Untracked) {
        vec!["diff", "--no-index", "--", "/dev/null", &file.path]
    } else if file.staged {
        vec!["diff", "--cached", "--", &file.path]
    } else {
        vec!["diff", "--", &file.path]
    };
    // `--no-index` reports a difference with exit code 1, so this one
    // can't go through the success-gated helper.
    let raw = match git_command(location, &args).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => {
            return DiffResult {
                text: String::from("Failed to load diff"),
                ..Default::default()
            };
        }
    };
    let text = truncate_diff(strip_diff_headers(&raw));
    let highlights = highlight_diff(&text, &file.path);
    let word_ranges = word_diff_ranges(&text);
    DiffResult {
        text,
        highlights,
        word_ranges,
    }
}

/// Drop everything before the first hunk (the `diff --git`/index/+++/---
/// preamble) and the `@@` hunk headers themselves — the view shows the
/// changed lines, not git's framing.
fn strip_diff_headers(raw: &str) -> String {
    raw.lines()
        .skip_while(|l| !l.starts_with("@@"))
        .filter(|l| !l.starts_with("@@"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_diff(mut text: String) -> String {
    if text.len() > MAX_DIFF_BYTES {
        let mut cut = MAX_DIFF_BYTES;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n\n... (truncated — diff exceeds 256 KB)");
        return text;
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > MAX_DIFF_LINES {
        let mut truncated: String = lines[..MAX_DIFF_LINES].join("\n");
        truncated.push_str("\n\n... (truncated — diff exceeds 5000 lines)");
        return truncated;
    }
    text
}

/// Minified files (bundled JS/CSS) have one logical line that's tens or
/// hundreds of KB long. syntect emits thousands of style spans for such a
/// line, and applying them chokes whichever toolkit receives them. Skip
/// highlighting entirely — the `+`/`-` bands still convey the diff.
const MAX_HIGHLIGHTABLE_LINE: usize = 2000;

pub fn highlight_diff(text: &str, file_path: &str) -> Vec<(usize, usize, usize, String)> {
    use syntect::easy::HighlightLines;

    if text.lines().any(|l| l.len() > MAX_HIGHLIGHTABLE_LINE) {
        return Vec::new();
    }

    let ss = &*SYNTAX_SET;
    let ts = &*THEME_SET;
    let theme = &ts.themes["base16-eighties.dark"];

    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let syntax = ss
        .find_syntax_by_extension(ext)
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut h = HighlightLines::new(syntax, theme);

    let mut highlights = Vec::new();
    for (line_idx, line) in text.lines().enumerate() {
        // Highlight the CODE, not git's leading marker column — the
        // offsets are shifted back by the marker so they still index
        // into the full line the caller holds.
        let (prefix_len, code) = match line.as_bytes().first() {
            Some(b'+') if !line.starts_with("+++") => (1, &line[1..]),
            Some(b'-') if !line.starts_with("---") => (1, &line[1..]),
            Some(b' ') => (1, &line[1..]),
            _ => continue,
        };

        if let Ok(ranges) = h.highlight_line(code, ss) {
            let mut byte_offset = prefix_len;
            for (style, token) in ranges {
                if !token.is_empty() {
                    let color = format!(
                        "#{:02x}{:02x}{:02x}",
                        style.foreground.r, style.foreground.g, style.foreground.b
                    );
                    highlights.push((line_idx, byte_offset, token.len(), color));
                }
                byte_offset += token.len();
            }
        }
    }
    highlights
}

/// Below this share of the shorter line's tokens in common, a `-`/`+` pair
/// isn't a rewrite of one line — it's two unrelated lines that happen to be
/// adjacent, and marking "what changed" in them highlights nearly the whole
/// pair, which is worse than marking nothing.
const WORD_DIFF_SIMILARITY: f32 = 0.35;

/// Pairs longer than this skip word-diffing. The LCS below is O(n·m), and
/// a 5000-line diff can hold ~2500 pairs; without a cap, one minified line
/// would lock the worker thread that called `load_diff`.
const MAX_WORD_DIFF_TOKENS: usize = 256;

/// Beyond this many candidate pairs, the two runs are matched index-wise
/// instead of scored against each other. Scoring is only worth its cost on
/// the run sizes people actually read.
const MAX_ALIGN_CELLS: usize = 4096;

/// How much a candidate pair is discounted for sitting far from the
/// diagonal, at most this fraction of its score at the extreme ends of
/// the two runs. Enough to break ties in favour of the obvious pairing
/// without stopping a genuinely better match a few rows away from winning.
const ALIGN_LOCALITY: f32 = 0.5;

/// Find the parts of each `-`/`+` pair that actually changed.
///
/// Within a hunk, a run of removed lines followed by a run of added lines
/// is git's shape for "these lines were rewritten". The two runs are
/// ALIGNED before diffing (see [`align_runs`]) and each resulting pair is
/// then diffed by *token*, so the view can mark the handful of words that
/// moved rather than lighting the whole line.
pub fn word_diff_ranges(text: &str) -> Vec<(usize, usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        if kind_of(lines[i]) != Some(false) {
            i += 1;
            continue;
        }
        let dels = i;
        let mut j = i;
        while j < lines.len() && kind_of(lines[j]) == Some(false) {
            j += 1;
        }
        let adds = j;
        while j < lines.len() && kind_of(lines[j]) == Some(true) {
            j += 1;
        }

        for (k, l) in align_runs(&lines[dels..adds], &lines[adds..j]) {
            let (di, ai) = (dels + k, adds + l);
            // Skip the marker byte; the ranges are shifted back by it so
            // they still index into the full line.
            if let Some((d, a)) = changed_ranges(&lines[di][1..], &lines[ai][1..]) {
                out.extend(d.into_iter().map(|(o, l)| (di, o + 1, l)));
                out.extend(a.into_iter().map(|(o, l)| (ai, o + 1, l)));
            }
        }
        // `j` is past the added run; if there was no added run it is one
        // past the removed one, so this always advances.
        i = j.max(i + 1);
    }
    out.sort_unstable();
    out
}

/// `Some(true)` for an added line, `Some(false)` for a removed one, `None`
/// for context. `+++`/`---` are file headers, not content.
fn kind_of(line: &str) -> Option<bool> {
    match line.as_bytes().first() {
        Some(b'+') if !line.starts_with("+++") => Some(true),
        Some(b'-') if !line.starts_with("---") => Some(false),
        _ => None,
    }
}

/// Decide which removed line answers which added line.
///
/// Pairing the two runs index-wise is wrong the moment their lengths
/// disagree for a reason other than a 1:1 rewrite: inserting a comment at
/// the top of the added run shifts every line down by one, and each
/// removed line is then compared against its neighbour's replacement. That
/// mispairing is *invisible in the band* — both lines are fully coloured
/// either way — and shows up only as word marks on lines that have nothing
/// to do with each other.
///
/// So score every candidate pair and take the best ones greedily, keeping
/// the result monotonic (a pair may not cross one already accepted, or the
/// marks would claim the file's lines were reordered). Scoring uses cheap
/// multiset containment, not the LCS below — the expensive pass runs only
/// on the pairs this one chooses.
fn align_runs(dels: &[&str], adds: &[&str]) -> Vec<(usize, usize)> {
    let (n, m) = (dels.len(), adds.len());
    if n == 0 || m == 0 {
        return Vec::new();
    }
    if n * m > MAX_ALIGN_CELLS {
        return (0..n.min(m)).map(|k| (k, k)).collect();
    }

    // Every line here carries a `-`/`+` marker byte; the tokens are of the
    // code, so the marker never counts toward similarity.
    let dt: Vec<_> = dels.iter().map(|l| tokens(&l[1..])).collect();
    let at: Vec<_> = adds.iter().map(|l| tokens(&l[1..])).collect();

    let span = n.max(m) as f32;
    let mut cands: Vec<(f32, usize, usize)> = Vec::new();
    for (i, d) in dt.iter().enumerate() {
        for (j, a) in at.iter().enumerate() {
            // Rewrites are LOCAL: git emitted these runs in order, so a
            // line five rows down has to be substantially more similar to
            // beat the one sitting opposite. Without this, a couple of
            // shared punctuation tokens are enough for a distant line to
            // steal the pairing — and the marks then describe a rewrite
            // that never happened.
            let locality = 1.0 - ALIGN_LOCALITY * (i.abs_diff(j) as f32 / span);
            let score = containment(d, a) * locality;
            if score >= WORD_DIFF_SIMILARITY {
                cands.push((score, i, j));
            }
        }
    }
    // Best first; ties go to the pair that moves the line least, then to
    // the earlier line, so the result doesn't depend on sort stability.
    cands.sort_by(|x, y| {
        y.0.total_cmp(&x.0)
            .then_with(|| x.1.abs_diff(x.2).cmp(&y.1.abs_diff(y.2)))
            .then_with(|| x.1.cmp(&y.1))
    });

    let mut out: Vec<(usize, usize)> = Vec::new();
    for (_, i, j) in cands {
        let conflicts = out
            .iter()
            .any(|&(pi, pj)| pi == i || pj == j || (i > pi) != (j > pj));
        if !conflicts {
            out.push((i, j));
        }
    }
    out.sort_unstable();
    out
}

/// How much of the shorter line's token multiset the other one contains.
///
/// A multiset, not a set: a line repeating `0` four times should not read
/// as similar to one using it once. Whitespace is excluded outright —
/// every line in a run shares its indentation, so counting it scores two
/// deeply-nested lines as similar for a reason that has nothing to do
/// with what they say. Leaving it in is what let `filter: drop-shadow(0 0
/// 2px rgb(…))` match `box-shadow: inset 0 0 0 1px var(…)` (six of its
/// fifteen shared tokens were spaces) over the `filter: var(…)` that
/// actually replaced it.
fn solid<'t>(toks: &[(usize, &'t str)]) -> Vec<&'t str> {
    toks.iter()
        .map(|&(_, t)| t)
        .filter(|t| !t.trim().is_empty())
        .collect()
}

fn containment(a: &[(usize, &str)], b: &[(usize, &str)]) -> f32 {
    let (a, b) = (solid(a), solid(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for token in &a {
        *counts.entry(token).or_default() += 1;
    }
    let mut shared = 0usize;
    for token in &b {
        if let Some(left) = counts.get_mut(token).filter(|c| **c > 0) {
            *left -= 1;
            shared += 1;
        }
    }
    shared as f32 / a.len().min(b.len()) as f32
}

/// Split into words, whitespace runs, and single punctuation characters —
/// the granularity that makes `, [data-highlighted]` read as one insertion
/// rather than a scatter of letters.
fn tokens(s: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let start = i;
        let c = s[i..].chars().next().expect("i is a char boundary");
        let class = |ch: char| (ch.is_alphanumeric() || ch == '_', ch.is_whitespace());
        let (word, space) = class(c);
        if word || space {
            while let Some(ch) = s[i..].chars().next() {
                let (w, sp) = class(ch);
                if w == word && sp == space {
                    i += ch.len_utf8();
                } else {
                    break;
                }
            }
        } else {
            i += c.len_utf8();
        }
        out.push((start, &s[start..i]));
    }
    out
}

/// Token-level LCS of two lines, returned as the `(offset, len)` byte
/// ranges of what did NOT survive on each side. `None` when the pair is
/// too dissimilar to be worth marking.
#[allow(clippy::type_complexity)]
fn changed_ranges(a: &str, b: &str) -> Option<(Vec<(usize, usize)>, Vec<(usize, usize)>)> {
    let (ta, tb) = (tokens(a), tokens(b));
    let (n, m) = (ta.len(), tb.len());
    if n.max(m) > MAX_WORD_DIFF_TOKENS {
        return None;
    }

    // dp[i][j] = length of the LCS of ta[i..] and tb[j..], flattened.
    let stride = m + 1;
    let mut dp = vec![0u16; (n + 1) * stride];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * stride + j] = if ta[i].1 == tb[j].1 {
                dp[(i + 1) * stride + j + 1] + 1
            } else {
                dp[(i + 1) * stride + j].max(dp[i * stride + j + 1])
            };
        }
    }
    // Measured against the SHORTER side. A rewrite that replaces a long
    // expression with a short one ("drop-shadow(0 0 2px rgb(…))" becoming
    // "var(--glow)") keeps almost all of the short side and only a quarter
    // of the long one, so scoring against the longer side rejects exactly
    // the pairs worth marking: on real samples /max put it at 0.23 next to
    // an unrelated pair's 0.06, while /min separates them 0.41 to 0.10.
    if f32::from(dp[0]) < n.min(m).max(1) as f32 * WORD_DIFF_SIMILARITY {
        return None;
    }

    let (mut keep_a, mut keep_b) = (vec![false; n], vec![false; m]);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if ta[i].1 == tb[j].1 {
            keep_a[i] = true;
            keep_b[j] = true;
            i += 1;
            j += 1;
        } else if dp[(i + 1) * stride + j] >= dp[i * stride + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    Some((runs(&ta, &keep_a), runs(&tb, &keep_b)))
}

/// Collapse a keep-mask into the byte ranges it leaves uncovered, merging
/// adjacent unkept tokens. A run that is only whitespace is dropped — a
/// lone marked space reads as a rendering glitch, not as a change.
fn runs(toks: &[(usize, &str)], keep: &[bool]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (k, &(offset, tok)) in toks.iter().enumerate() {
        if keep[k] {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.0 + last.1 == offset => last.1 += tok.len(),
            _ => out.push((offset, tok.len())),
        }
    }
    out.retain(|&(o, l)| {
        toks.iter()
            .any(|&(to, t)| to >= o && to < o + l && !t.trim().is_empty())
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_ahead_behind_and_changes() {
        let text = "\
# branch.oid 0123abc\n\
# branch.head iced-migration\n\
# branch.upstream origin/iced-migration\n\
# branch.ab +2 -1\n\
1 .M N... 100644 100644 100644 0a 0b src/main.rs\n\
? target/new-file\n";
        let status = parse_porcelain_v2(text);
        assert_eq!(status.branch, "iced-migration");
        assert_eq!(status.ahead, 2);
        assert_eq!(status.behind, 1);
        assert_eq!(status.changed, 2);
    }

    /// Detached HEAD reports "(detached)"; clean tree counts zero.
    #[test]
    fn parses_detached_clean() {
        let text = "# branch.oid 0123abc\n# branch.head (detached)\n";
        let status = parse_porcelain_v2(text);
        assert_eq!(status.branch, "(detached)");
        assert_eq!(status.changed, 0);
        assert_eq!(status.ahead, 0);
    }

    #[test]
    fn parses_full_shortstat() {
        assert_eq!(
            parse_shortstat(" 3 files changed, 120 insertions(+), 45 deletions(-)"),
            (3, 120, 45)
        );
    }

    /// git omits the clauses that would be zero — an all-additions diff
    /// has no deletion clause at all, and a clean tree prints nothing.
    #[test]
    fn shortstat_clauses_are_optional() {
        assert_eq!(
            parse_shortstat(" 1 file changed, 7 insertions(+)"),
            (1, 7, 0)
        );
        assert_eq!(
            parse_shortstat(" 2 files changed, 9 deletions(-)"),
            (2, 0, 9)
        );
        assert_eq!(parse_shortstat(""), (0, 0, 0));
    }

    #[test]
    fn parses_porcelain_v1_status_columns() {
        let cases = [
            ("?? notes.txt", FileStatus::Untracked, false, "notes.txt"),
            (" M src/main.rs", FileStatus::Modified, false, "src/main.rs"),
            ("M  src/main.rs", FileStatus::Modified, true, "src/main.rs"),
            ("A  new.rs", FileStatus::Added, true, "new.rs"),
            (" D gone.rs", FileStatus::Deleted, false, "gone.rs"),
            ("R  a -> b", FileStatus::Renamed, true, "a -> b"),
        ];
        for (line, status, staged, path) in cases {
            let file = parse_status_line(line).expect(line);
            assert_eq!(file.status, status, "{line}");
            assert_eq!(file.staged, staged, "{line}");
            assert_eq!(file.path, path, "{line}");
        }
        // Too short to carry a path, and an unhandled column pair.
        assert!(parse_status_line("M").is_none());
        assert!(parse_status_line("XY thing").is_none());
    }

    /// The view shows changed lines, not git's framing — everything
    /// before the first hunk goes, and the `@@` headers with it.
    #[test]
    fn strips_preamble_and_hunk_headers() {
        let raw = "diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n\
@@ -1,2 +1,2 @@\n-old\n+new\n@@ -9,1 +9,1 @@\n context\n";
        assert_eq!(strip_diff_headers(raw), "-old\n+new\n context");
    }

    /// A diff with no hunks at all (binary files, mode-only changes)
    /// must come back empty rather than carrying the preamble through.
    #[test]
    fn strips_everything_when_there_is_no_hunk() {
        let raw = "diff --git a/logo.png b/logo.png\nBinary files differ\n";
        assert_eq!(strip_diff_headers(raw), "");
    }

    /// The byte cap has to land on a char boundary — a multi-byte glyph
    /// straddling it would panic the truncate.
    #[test]
    fn byte_cap_respects_char_boundaries() {
        let text = "é".repeat(MAX_DIFF_BYTES);
        let out = truncate_diff(text);
        assert!(out.ends_with("256 KB)"), "should be byte-truncated");
        assert!(out.is_char_boundary(MAX_DIFF_BYTES.min(out.len())));
    }

    #[test]
    fn line_cap_truncates_long_diffs() {
        let text = "+line\n".repeat(MAX_DIFF_LINES + 10);
        let out = truncate_diff(text);
        assert!(out.contains("exceeds 5000 lines"), "{out}");
        assert_eq!(
            out.lines().filter(|l| *l == "+line").count(),
            MAX_DIFF_LINES
        );
    }

    /// A minified file is one enormous line; syntect would emit thousands
    /// of spans for it, so highlighting is skipped wholesale.
    #[test]
    fn skips_highlighting_minified_lines() {
        let text = format!("+{}", "a".repeat(MAX_HIGHLIGHTABLE_LINE + 1));
        assert!(highlight_diff(&text, "bundle.js").is_empty());
    }

    /// Offsets must index into the FULL line (including git's marker
    /// column), not into the code the highlighter actually saw.
    #[test]
    fn highlight_offsets_skip_the_marker_column() {
        let marks = highlight_diff("+let x = 1;", "main.rs");
        assert!(!marks.is_empty(), "rust should highlight");
        assert!(
            marks.iter().all(|(line, off, len, _)| {
                *line == 0 && *off >= 1 && off + len <= "+let x = 1;".len()
            }),
            "offsets must stay inside the line and past the marker: {marks:?}"
        );
    }

    /// Slice a result back out of the text it describes, so the assertions
    /// below read as "these are the words that changed".
    fn marked(text: &str) -> Vec<String> {
        let lines: Vec<&str> = text.lines().collect();
        word_diff_ranges(text)
            .into_iter()
            .map(|(l, o, n)| lines[l][o..o + n].to_string())
            .collect()
    }

    /// The case the whole feature exists for: a pair differing by one
    /// inserted clause should mark that clause and nothing else.
    #[test]
    fn marks_only_the_inserted_clause() {
        let text = "\
-    .sidebar-link-root:is(:hover, :focus-visible) .sidebar-link-label {\n\
+    .sidebar-link-root:is(:hover, :focus-visible, [data-highlighted]) .sidebar-link-label {";
        assert_eq!(marked(text), vec![", [data-highlighted]"]);
    }

    /// Offsets must land past git's marker, like `highlight_diff`'s do —
    /// `marked()` slicing the right words is what proves it.
    #[test]
    fn marks_a_replaced_word_on_both_sides() {
        let text = "-let total = count;\n+let total = amount;";
        assert_eq!(marked(text), vec!["count", "amount"]);
    }

    /// Two adjacent but unrelated lines share almost no tokens; marking
    /// them would light up the whole pair, so the pair is left alone.
    #[test]
    fn leaves_dissimilar_pairs_unmarked() {
        let text = "-use std::collections::BTreeMap;\n+fn main() { println!(\"hi\") }";
        assert!(word_diff_ranges(text).is_empty());
    }

    /// Runs pair up index-wise, and a run with no partner is skipped
    /// rather than pairing across the gap. Results come back in line
    /// order, so the two sides of a pair are not adjacent here.
    #[test]
    fn pairs_runs_index_wise() {
        let text =
            "-let a = 1;\n-let b = 2;\n+let a = 9;\n+let b = 8;\n context\n-orphan line here";
        assert_eq!(marked(text), vec!["1", "2", "9", "8"]);
    }

    /// An inserted comment makes the added run one longer than the removed
    /// one. Pairing index-wise then compares each removed line against its
    /// neighbour's replacement, which marked `@apply` and `grey-100` on a
    /// line whose partner was a `text-shadow:` declaration — caught on
    /// screen, not by a unit test, because the band looks identical either
    /// way.
    #[test]
    fn an_inserted_line_does_not_shift_the_pairing() {
        let text = "\
-    .sidebar-link-root:is(:hover) .sidebar-link-label {\n\
-        @apply text-orange-light;\n\
-        text-shadow: 0 0 6px rgb(222 82 0 / 0.45);\n\
+    /* Figma \"_Sidebar-Items\" states. */\n\
+    .sidebar-link-root:is(:hover, [data-highlighted]) .sidebar-link-label {\n\
+        @apply text-grey-100;\n\
+        text-shadow:";
        let marks = marked(text);
        assert!(
            marks.contains(&String::from(", [data-highlighted]")),
            "the selector pair should still be found: {marks:?}"
        );
        // The two `@apply` lines pair with each other, so only the colour
        // words differ. `text-` and the hyphens are common to both, which
        // is why these are marked as separate words rather than as
        // `orange-light` / `grey-100`.
        for word in ["orange", "light", "grey", "100"] {
            assert!(
                marks.iter().any(|m| m == word),
                "{word:?} should be marked: {marks:?}"
            );
        }
        assert!(
            !marks.iter().any(|m| m.contains("@apply")),
            "@apply is common to both sides and must not be marked: {marks:?}"
        );
    }

    /// A distant line sharing only punctuation and indentation must not
    /// steal a pairing from the line sitting opposite. Also caught on
    /// screen: `filter: drop-shadow(…)` paired with the `box-shadow:`
    /// five rows below it, leaving the `filter: var(…)` that actually
    /// replaced it unmarked.
    #[test]
    fn a_distant_lookalike_does_not_steal_the_pairing() {
        let text = "\
-        filter: drop-shadow(0 0 2px rgb(222 82 0 / 0.45));\n\
+        filter: var(--filter-icon-tertiary-glow);\n\
+    }\n\
+\n\
+    .sidebar-row:has(:focus-visible) {\n\
+        box-shadow: inset 0 0 0 1px var(--color-orange-light);";
        let lines: Vec<&str> = text.lines().collect();
        let marked_lines: Vec<usize> = word_diff_ranges(text)
            .into_iter()
            .map(|(l, ..)| l)
            .collect();
        assert!(
            marked_lines.contains(&1),
            "the filter: line opposite it should be marked: {marked_lines:?}"
        );
        assert!(
            !marked_lines.contains(&5),
            "the distant box-shadow line must not be paired: {:?}",
            lines[5]
        );
    }

    /// Alignment must not claim lines were reordered: a pair that crosses
    /// one already chosen is rejected, however well it scores.
    #[test]
    fn alignment_stays_monotonic() {
        let pairs = align_runs(&["-alpha one", "-beta two"], &["+beta two!", "+alpha one!"]);
        assert!(
            pairs.windows(2).all(|w| w[0].1 < w[1].1),
            "second components must increase: {pairs:?}"
        );
    }

    /// A lone changed space would render as a one-column smear with no
    /// readable content, so whitespace-only runs are dropped.
    #[test]
    fn drops_whitespace_only_changes() {
        let text = "-let x = 1;\n+let x =  1;";
        assert!(
            word_diff_ranges(text).is_empty(),
            "an added space is not worth marking"
        );
    }

    /// File headers start with the same bytes as content lines and must
    /// not be mistaken for a rewritten pair.
    #[test]
    fn ignores_file_header_lines() {
        let text = "--- a/src/main.rs\n+++ b/src/main.rs";
        assert!(word_diff_ranges(text).is_empty());
    }

    /// The O(n·m) guard: a minified pair must bail rather than lock the
    /// worker thread that called `load_diff`.
    #[test]
    fn skips_pairs_with_too_many_tokens() {
        let long: String = (0..MAX_WORD_DIFF_TOKENS + 10)
            .map(|i| format!("t{i} "))
            .collect();
        let text = format!("-{long}\n+{long}x");
        assert!(word_diff_ranges(&text).is_empty());
    }

    /// Every range must be sliceable out of its own line — the view does
    /// exactly this, and an off-by-one here is a panic there.
    #[test]
    fn ranges_stay_inside_their_line() {
        let text = "-    filter: drop-shadow(0 0 2px rgb(222 82 0 / 0.45));\n\
+    filter: var(--filter-icon-tertiary-glow);\n\
 unchanged";
        let lines: Vec<&str> = text.lines().collect();
        let ranges = word_diff_ranges(text);
        assert!(!ranges.is_empty());
        for (l, o, n) in ranges {
            let line = lines[l];
            assert!(o >= 1, "offset must clear the marker");
            assert!(o + n <= line.len(), "range past end of line {l}");
            assert!(line.is_char_boundary(o) && line.is_char_boundary(o + n));
        }
    }
}
