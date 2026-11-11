//! Inline parser: lexer (atoms + delimiter runs) → delimiter-stack resolution → tree build.

use dn_core::Id;

use super::{Inline, MAX_DEPTH};
use crate::util::{find_tick_run, run_len};

/// Longest accepted URL (Discord's own limit is 2048).
const MAX_URL: usize = 2048;

const SPECIAL: [bool; 256] = {
    let mut t = [false; 256];
    let set = b"\n\\`<[hH@*_~|";
    let mut i = 0;
    while i < set.len() {
        t[set[i] as usize] = true;
        i += 1;
    }
    t
};

pub fn parse_inline(src: &str) -> Vec<Inline> {
    parse_with(src, true, 0)
}

fn parse_with(src: &str, links: bool, depth: usize) -> Vec<Inline> {
    if src.is_empty() {
        return Vec::new();
    }
    let mut lx = Lexer::new(src, links, depth);
    lx.lex();
    resolve(&mut lx.delims);
    build(lx)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wrap {
    Italic,
    Bold,
    Underline,
    Strike,
    Spoiler,
}

impl Wrap {
    fn wrap(self, kids: Vec<Inline>) -> Inline {
        match self {
            Wrap::Italic => Inline::Italic(kids),
            Wrap::Bold => Inline::Bold(kids),
            Wrap::Underline => Inline::Underline(kids),
            Wrap::Strike => Inline::Strike(kids),
            Wrap::Spoiler => Inline::Spoiler(kids),
        }
    }
}

enum Tok {
    /// Byte range of the source rendered verbatim.
    Text(usize, usize),
    Atom(usize),
    Delim(usize),
    Break,
}

struct Delim {
    ch: u8,
    orig: usize,
    remaining: usize,
    can_open: bool,
    can_close: bool,
    used_close: usize,
    used_open: usize,
    /// Containers opened/closed by this run, innermost first.
    opens: Vec<Wrap>,
    closes: Vec<Wrap>,
}

impl Delim {
    /// Whether the remaining characters can still take part in a match.
    fn viable(&self) -> bool {
        match self.ch {
            b'~' | b'|' => self.remaining >= 2,
            _ => self.remaining >= 1,
        }
    }
}

struct Lexer<'a> {
    src: &'a str,
    b: &'a [u8],
    links: bool,
    depth: usize,
    toks: Vec<Tok>,
    atoms: Vec<Inline>,
    delims: Vec<Delim>,
    /// Scan allowance for failed URL searches, keeping pathological input linear.
    budget: usize,
    brackets: Option<Vec<u32>>,
    tick_fail: [bool; 4],
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str, links: bool, depth: usize) -> Self {
        Lexer {
            src,
            b: src.as_bytes(),
            links,
            depth,
            toks: Vec::new(),
            atoms: Vec::new(),
            delims: Vec::new(),
            budget: src.len().saturating_mul(16).saturating_add(1024),
            brackets: None,
            tick_fail: [false; 4],
        }
    }

    fn text(&mut self, from: usize, to: usize) {
        if to > from {
            self.toks.push(Tok::Text(from, to));
        }
    }

    fn atom(&mut self, i: Inline) {
        self.atoms.push(i);
        self.toks.push(Tok::Atom(self.atoms.len() - 1));
    }

    fn lex(&mut self) {
        let b = self.b;
        let n = b.len();
        let mut i = 0;
        let mut ts = 0;
        while i < n {
            let c = b[i];
            if !SPECIAL[c as usize] {
                i += 1;
                continue;
            }
            match c {
                b'\n' => {
                    self.text(ts, i);
                    self.toks.push(Tok::Break);
                    i += 1;
                    ts = i;
                }
                b'\\' => match self.src[i + 1..].chars().next() {
                    Some(ch) if !ch.is_alphanumeric() && !ch.is_whitespace() => {
                        self.text(ts, i);
                        i += 1;
                        ts = i;
                        i += ch.len_utf8();
                    }
                    _ => i += 1,
                },
                b'`' => {
                    let run = run_len(b, i, b'`');
                    match self.code_span(i, run) {
                        Some((code, end)) => {
                            self.text(ts, i);
                            self.atom(code);
                            i = end;
                            ts = i;
                        }
                        None => i += run,
                    }
                }
                b'<' => match self.angle(i) {
                    Some((atom, end)) => {
                        self.text(ts, i);
                        self.atom(atom);
                        i = end;
                        ts = i;
                    }
                    None => i += 1,
                },
                b'[' => match self.link(i) {
                    Some((atom, end)) => {
                        self.text(ts, i);
                        self.atom(atom);
                        i = end;
                        ts = i;
                    }
                    None => i += 1,
                },
                b'h' | b'H' => match self.autolink(i) {
                    Some((atom, end)) => {
                        self.text(ts, i);
                        self.atom(atom);
                        i = end;
                        ts = i;
                    }
                    None => i += 1,
                },
                b'@' => match self.broadcast(i) {
                    Some((atom, end)) => {
                        self.text(ts, i);
                        self.atom(atom);
                        i = end;
                        ts = i;
                    }
                    None => i += 1,
                },
                _ => {
                    // `*`, `_`, `~`, `|`
                    let run = run_len(b, i, c);
                    let min = if matches!(c, b'~' | b'|') { 2 } else { 1 };
                    if run < min {
                        i += run;
                        continue;
                    }
                    let prev = self.src[..i].chars().next_back();
                    let next = self.src[i + run..].chars().next();
                    let (can_open, can_close) = flanking(c, run, prev, next);
                    if !can_open && !can_close {
                        i += run;
                        continue;
                    }
                    self.text(ts, i);
                    self.delims.push(Delim {
                        ch: c,
                        orig: run,
                        remaining: run,
                        can_open,
                        can_close,
                        used_close: 0,
                        used_open: 0,
                        opens: Vec::new(),
                        closes: Vec::new(),
                    });
                    self.toks.push(Tok::Delim(self.delims.len() - 1));
                    i += run;
                    ts = i;
                }
            }
        }
        self.text(ts, n);
    }

    fn code_span(&mut self, i: usize, run: usize) -> Option<(Inline, usize)> {
        if run > 3 || self.tick_fail[run] {
            return None;
        }
        let Some(close) = find_tick_run(self.b, i + run, run) else {
            self.tick_fail[run] = true;
            return None;
        };
        let inner = &self.src[i + run..close];
        let padded = inner.len() >= 2 && inner.starts_with(' ') && inner.ends_with(' ') && !inner.trim().is_empty();
        let code = if padded { &inner[1..inner.len() - 1] } else { inner };
        Some((Inline::Code(code.to_string()), close + run))
    }

    fn broadcast(&self, i: usize) -> Option<(Inline, usize)> {
        let rest = &self.b[i + 1..];
        let (word, atom) = if rest.starts_with(b"everyone") {
            (8, Inline::Everyone)
        } else if rest.starts_with(b"here") {
            (4, Inline::Here)
        } else {
            return None;
        };
        let end = i + 1 + word;
        let prev_ok = !self.src[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_');
        let next_ok = !self.src[end..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_');
        (prev_ok && next_ok).then_some((atom, end))
    }

    fn angle(&mut self, i: usize) -> Option<(Inline, usize)> {
        let b = self.b;
        match *b.get(i + 1)? {
            b'@' => {
                let mut j = i + 2;
                let role = b.get(j) == Some(&b'&');
                if role || b.get(j) == Some(&b'!') {
                    j += 1;
                }
                let (id, e) = id_at(b, j)?;
                if b.get(e) != Some(&b'>') {
                    return None;
                }
                Some((if role { Inline::RoleMention(id) } else { Inline::UserMention(id) }, e + 1))
            }
            b'#' => {
                let (id, e) = id_at(b, i + 2)?;
                (b.get(e) == Some(&b'>')).then_some((Inline::ChannelMention(id), e + 1))
            }
            b':' => custom_emoji_at(self.src, i),
            b'a' if b.get(i + 2) == Some(&b':') => custom_emoji_at(self.src, i),
            b't' if b.get(i + 2) == Some(&b':') => timestamp_at(b, i),
            b'/' => self.slash_command(i),
            b'h' | b'H' if self.links => self.angle_link(i),
            _ => None,
        }
    }

    fn slash_command(&self, i: usize) -> Option<(Inline, usize)> {
        let b = self.b;
        let ns = i + 2;
        let mut j = ns;
        while j < b.len() && j - ns <= 100 && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'_' | b'-' | b' ') || b[j] >= 0x80) {
            j += 1;
        }
        if b.get(j) != Some(&b':') {
            return None;
        }
        let name = &self.src[ns..j];
        let words = name.split(' ');
        if name.is_empty() || words.clone().count() > 3 || words.clone().any(|w| w.is_empty()) {
            return None;
        }
        let (id, e) = id_at(b, j + 1)?;
        (b.get(e) == Some(&b'>')).then(|| (Inline::SlashCommand { name: name.to_string(), id }, e + 1))
    }

    fn angle_link(&mut self, i: usize) -> Option<(Inline, usize)> {
        let b = self.b;
        let scheme = scheme_len(&b[i + 1..]);
        if scheme == 0 || self.budget == 0 {
            return None;
        }
        let limit = b.len().min(i + 1 + MAX_URL).min(i + 1 + self.budget);
        let mut j = i + 1 + scheme;
        while j < limit && b[j] != b'>' && b[j] != b'<' && !b[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= limit || b[j] != b'>' || j == i + 1 + scheme {
            self.budget = self.budget.saturating_sub(j - i);
            return None;
        }
        Some((Inline::AutoLink(self.src[i + 1..j].to_string()), j + 1))
    }

    fn autolink(&self, i: usize) -> Option<(Inline, usize)> {
        let b = self.b;
        if !self.links {
            return None;
        }
        let scheme = scheme_len(&b[i..]);
        if scheme == 0 {
            return None;
        }
        let mut end = i + scheme;
        while end < b.len() && !b[end].is_ascii_whitespace() && b[end] != b'<' {
            end += 1;
        }
        let len = trim_url_end(&self.src[i..end], scheme);
        (len > scheme).then(|| (Inline::AutoLink(self.src[i..i + len].to_string()), i + len))
    }

    fn bracket_close(&mut self, i: usize) -> Option<usize> {
        let b = self.b;
        if b.len() >= u32::MAX as usize {
            return None;
        }
        let table = self.brackets.get_or_insert_with(|| bracket_table(b));
        match table[i] {
            u32::MAX => None,
            c => Some(c as usize),
        }
    }

    fn link(&mut self, i: usize) -> Option<(Inline, usize)> {
        if !self.links || self.depth >= MAX_DEPTH {
            return None;
        }
        let close = self.bracket_close(i)?;
        if close == i + 1 || self.b.get(close + 1) != Some(&b'(') {
            return None;
        }
        let (url, end) = self.link_dest(close + 2)?;
        let text = parse_with(&self.src[i + 1..close], false, self.depth + 1);
        Some((Inline::Link { text, url }, end))
    }

    fn link_dest(&mut self, from: usize) -> Option<(String, usize)> {
        if self.budget == 0 {
            return None;
        }
        let limit = self.b.len().min(from + MAX_URL + 64).min(from + self.budget);
        let found = scan_dest(self.src, from, limit);
        if found.is_none() {
            self.budget = self.budget.saturating_sub(limit - from);
        }
        found
    }
}

fn flanking(c: u8, run: usize, prev: Option<char>, next: Option<char>) -> (bool, bool) {
    let next_ok = next.is_some_and(|c| !c.is_whitespace());
    let prev_ok = prev.is_some_and(|c| !c.is_whitespace());
    match c {
        b'|' => (true, true),
        // Single underscores only count at word boundaries (`snake_case` stays literal).
        b'_' if run == 1 => (next_ok && !prev.is_some_and(char::is_alphanumeric), prev_ok && !next.is_some_and(char::is_alphanumeric)),
        _ => (next_ok, prev_ok),
    }
}

fn scheme_len(b: &[u8]) -> usize {
    let ci = |p: &[u8]| b.len() >= p.len() && b[..p.len()].eq_ignore_ascii_case(p);
    if ci(b"https://") {
        8
    } else if ci(b"http://") {
        7
    } else {
        0
    }
}

/// Length of `url` after dropping trailing punctuation and unbalanced closers that belong to
/// the surrounding prose (never shorter than `min`).
fn trim_url_end(url: &str, min: usize) -> usize {
    let b = url.as_bytes();
    let count = |c: u8| b.iter().filter(|&&x| x == c).count();
    let (lp, mut rp, lb, mut rb) = (count(b'('), count(b')'), count(b'['), count(b']'));
    let mut end = b.len();
    while end > min {
        let drop = match b[end - 1] {
            b'.' | b',' | b':' | b';' | b'!' | b'?' | b'\'' | b'"' | b'*' | b'_' | b'~' | b'|' => true,
            b')' if rp > lp => {
                rp -= 1;
                true
            }
            b']' if rb > lb => {
                rb -= 1;
                true
            }
            _ => false,
        };
        if !drop {
            break;
        }
        end -= 1;
    }
    end
}

fn id_at(b: &[u8], from: usize) -> Option<(Id, usize)> {
    let len = b.get(from..)?.iter().take_while(|c| c.is_ascii_digit()).count();
    if len == 0 || len > 20 {
        return None;
    }
    let s = std::str::from_utf8(&b[from..from + len]).ok()?;
    Some((Id(s.parse().ok()?), from + len))
}

/// `<:name:id>` / `<a:name:id>` with `i` pointing at `<`.
pub(crate) fn custom_emoji_at(s: &str, i: usize) -> Option<(Inline, usize)> {
    let b = s.as_bytes();
    let mut j = i + 1;
    let animated = b.get(j) == Some(&b'a');
    if animated {
        j += 1;
    }
    if b.get(j) != Some(&b':') {
        return None;
    }
    j += 1;
    let ns = j;
    while j < b.len() && j - ns <= 32 && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
        j += 1;
    }
    if !(2..=32).contains(&(j - ns)) || b.get(j) != Some(&b':') {
        return None;
    }
    let (id, e) = id_at(b, j + 1)?;
    (b.get(e) == Some(&b'>')).then(|| (Inline::CustomEmoji { name: s[ns..j].to_string(), id, animated }, e + 1))
}

fn timestamp_at(b: &[u8], i: usize) -> Option<(Inline, usize)> {
    let mut j = i + 3;
    let neg = b.get(j) == Some(&b'-');
    if neg {
        j += 1;
    }
    let ds = j;
    while j < b.len() && b[j].is_ascii_digit() && j - ds <= 16 {
        j += 1;
    }
    if j == ds || j - ds > 16 {
        return None;
    }
    let mag: i64 = std::str::from_utf8(&b[ds..j]).ok()?.parse().ok()?;
    let unix = if neg { -mag } else { mag };
    let mut style = 'f';
    if b.get(j) == Some(&b':') {
        let st = *b.get(j + 1)?;
        if !b"tTdDfFR".contains(&st) {
            return None;
        }
        style = st as char;
        j += 2;
    }
    (b.get(j) == Some(&b'>')).then_some((Inline::Timestamp { unix, style }, j + 1))
}

/// Pairs every `[` with its `]` in one pass (escapes skipped); `u32::MAX` marks unmatched.
fn bracket_table(b: &[u8]) -> Vec<u32> {
    let mut table = vec![u32::MAX; b.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'[' => stack.push(i),
            b']' => {
                if let Some(open) = stack.pop() {
                    table[open] = i as u32;
                }
            }
            _ => {}
        }
        i += 1;
    }
    table
}

fn char_len_at(s: &str, i: usize) -> usize {
    s.get(i..).and_then(|r| r.chars().next()).map_or(1, char::len_utf8)
}

/// Parses `url)`, `<url>)` or `url "title")` starting right after the opening parenthesis.
fn scan_dest(src: &str, from: usize, limit: usize) -> Option<(String, usize)> {
    let b = src.as_bytes();
    let blank = |c: u8| c == b' ' || c == b'\t';
    let mut j = from;
    while j < limit && blank(b[j]) {
        j += 1;
    }
    let (url, mut j) = if b.get(j) == Some(&b'<') {
        let s = j + 1;
        let mut k = s;
        while k < limit && b[k] != b'>' && b[k] != b'<' && !b[k].is_ascii_whitespace() {
            k += 1;
        }
        if k >= limit || b[k] != b'>' {
            return None;
        }
        (&src[s..k], k + 1)
    } else {
        let s = j;
        let mut depth = 0usize;
        while j < limit {
            match b[j] {
                b'\\' => j += 1 + char_len_at(src, j + 1),
                b'(' => {
                    depth += 1;
                    j += 1;
                }
                b')' if depth == 0 => break,
                b')' => {
                    depth -= 1;
                    j += 1;
                }
                c if c.is_ascii_whitespace() => break,
                _ => j += 1,
            }
        }
        if j >= limit {
            return None;
        }
        (&src[s..j], j)
    };
    while j < limit && blank(b[j]) {
        j += 1;
    }
    if j < limit
        && let Some(&q @ (b'"' | b'\'')) = b.get(j)
    {
        let tlimit = limit.min(j + 256);
        let close = b[j + 1..tlimit].iter().position(|&c| c == q)?;
        j += close + 2;
        while j < limit && blank(b[j]) {
            j += 1;
        }
    }
    if j >= limit || b[j] != b')' {
        return None;
    }
    let sl = scheme_len(url.as_bytes());
    if sl == 0 || url.len() <= sl {
        return None;
    }
    Some((unescape(url), j + 1))
}

fn unescape(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some(n) if n.is_ascii_punctuation() => out.push(n),
                Some(n) => {
                    out.push('\\');
                    out.push(n);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn wrap_for(ch: u8, n: usize) -> Wrap {
    match (ch, n) {
        (b'*', 2) => Wrap::Bold,
        (b'_', 2) => Wrap::Underline,
        (b'~', _) => Wrap::Strike,
        (b'|', _) => Wrap::Spoiler,
        _ => Wrap::Italic,
    }
}

fn kind_index(ch: u8) -> usize {
    match ch {
        b'*' => 0,
        b'_' => 1,
        b'~' => 2,
        _ => 3,
    }
}

/// Whether `opener` may pair with `closer` (same character, rule of three for `*`/`_`).
fn pairs(opener: &Delim, closer: &Delim) -> bool {
    if opener.ch != closer.ch {
        return false;
    }
    match opener.ch {
        b'~' | b'|' => opener.remaining >= 2 && closer.remaining >= 2,
        _ => {
            // Rule of three: runs that can both open and close pair only if their lengths do not sum to a multiple of 3.
            let m = |n: usize| n % 3;
            let rejected =
                (closer.can_open || opener.can_close) && m(opener.orig + closer.orig) == 0 && !(m(opener.orig) == 0 && m(closer.orig) == 0);
            !rejected
        }
    }
}

/// CommonMark-style delimiter matching. Each closer scans only openers that were not already
/// rejected for its class (`bottoms`), so the whole pass is amortised linear.
fn resolve(delims: &mut [Delim]) {
    let mut stack: Vec<usize> = Vec::new();
    let mut bottoms = [0usize; 24];
    for ci in 0..delims.len() {
        if delims[ci].can_close {
            while delims[ci].viable() {
                let key = (kind_index(delims[ci].ch) * 2 + usize::from(delims[ci].can_open)) * 3 + delims[ci].orig % 3;
                let bottom = bottoms[key].min(stack.len());
                let found = (bottom..stack.len()).rev().find(|&si| pairs(&delims[stack[si]], &delims[ci]));
                let Some(si) = found else {
                    bottoms[key] = stack.len();
                    break;
                };
                let oi = stack[si];
                let ch = delims[ci].ch;
                let n = if matches!(ch, b'~' | b'|') || (delims[oi].remaining >= 2 && delims[ci].remaining >= 2) { 2 } else { 1 };
                let w = wrap_for(ch, n);
                delims[oi].opens.push(w);
                delims[oi].used_open += n;
                delims[oi].remaining -= n;
                delims[ci].closes.push(w);
                delims[ci].used_close += n;
                delims[ci].remaining -= n;
                stack.truncate(si + 1);
                if !delims[oi].viable() {
                    stack.pop();
                }
                for b in &mut bottoms {
                    *b = (*b).min(stack.len());
                }
            }
        }
        if delims[ci].can_open && delims[ci].viable() {
            stack.push(ci);
        }
    }
}

struct Frame {
    wrap: Option<Wrap>,
    kids: Vec<Inline>,
}

struct Builder {
    root: Vec<Inline>,
    frames: Vec<Frame>,
    real_depth: usize,
}

impl Builder {
    fn kids(&mut self) -> &mut Vec<Inline> {
        match self.frames.last_mut() {
            Some(f) => &mut f.kids,
            None => &mut self.root,
        }
    }

    fn push(&mut self, i: Inline) {
        let kids = self.kids();
        match (kids.last_mut(), i) {
            (Some(Inline::Text(t)), Inline::Text(s)) => t.push_str(&s),
            (_, i) => kids.push(i),
        }
    }

    fn text(&mut self, s: &str) {
        let kids = self.kids();
        match kids.last_mut() {
            Some(Inline::Text(t)) => t.push_str(s),
            _ => kids.push(Inline::Text(s.to_string())),
        }
    }

    fn open(&mut self, w: Wrap) {
        // Past the cap the marker is dropped and its contents are flattened.
        let wrap = if self.real_depth < MAX_DEPTH {
            self.real_depth += 1;
            Some(w)
        } else {
            None
        };
        self.frames.push(Frame { wrap, kids: Vec::new() });
    }

    fn close(&mut self) {
        let Some(f) = self.frames.pop() else { return };
        match f.wrap {
            None => {
                for k in f.kids {
                    self.push(k);
                }
            }
            Some(w) => {
                self.real_depth -= 1;
                if !f.kids.is_empty() {
                    self.push(w.wrap(f.kids));
                }
            }
        }
    }
}

fn build(lx: Lexer) -> Vec<Inline> {
    let Lexer { src, toks, atoms, delims, .. } = lx;
    let mut atoms: Vec<Option<Inline>> = atoms.into_iter().map(Some).collect();
    let mut bld = Builder { root: Vec::new(), frames: Vec::new(), real_depth: 0 };
    for tok in toks {
        match tok {
            Tok::Text(a, b) => bld.text(&src[a..b]),
            Tok::Break => bld.push(Inline::LineBreak),
            Tok::Atom(k) => {
                if let Some(a) = atoms[k].take() {
                    bld.push(a);
                }
            }
            Tok::Delim(k) => {
                let d = &delims[k];
                for _ in &d.closes {
                    bld.close();
                }
                let leftover = d.orig - d.used_close - d.used_open;
                if leftover > 0 {
                    bld.text(&String::from(d.ch as char).repeat(leftover));
                }
                for w in d.opens.iter().rev() {
                    bld.open(*w);
                }
            }
        }
    }
    while !bld.frames.is_empty() {
        bld.close();
    }
    bld.root
}
