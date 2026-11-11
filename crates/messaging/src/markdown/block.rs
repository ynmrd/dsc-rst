//! Block structure: fences, quotes, headings, subtext, lists and paragraphs.

use super::{Block, inline::parse_inline};
use crate::util::is_lang;

/// Nested list levels beyond this are flattened into the deepest list.
const MAX_LIST_DEPTH: usize = 16;

pub fn parse(src: &str) -> Vec<Block> {
    if src.contains('\r') {
        parse_blocks(&src.replace("\r\n", "\n").replace('\r', "\n"), false)
    } else {
        parse_blocks(src, false)
    }
}

/// `in_quote` disables nested quotes, as Discord does.
fn parse_blocks(src: &str, in_quote: bool) -> Vec<Block> {
    let len = src.len();
    // A fence can only close if another ``` follows it; checking against the last one keeps
    // unterminated fences from rescanning the tail.
    let last_fence = src.rfind("```");
    let mut out = Vec::new();
    let mut para: Option<usize> = None;
    let mut pos = 0;
    let mut mid = false;
    // Cached so lines holding many fences are not rescanned for their end each time.
    let mut end = 0;
    let mut have_end = false;
    while pos < len {
        if !have_end || end < pos {
            end = src[pos..].find('\n').map_or(len, |k| pos + k);
            have_end = true;
        }
        if !mid {
            let mut found = Vec::new();
            if let Some(next) = block_at(src, pos, end, in_quote, &mut found) {
                flush_para(&mut out, src, para.take(), pos);
                out.append(&mut found);
                pos = next;
                continue;
            }
        }
        if let Some((q, block, next)) = fence_in_line(src, pos, end, last_fence) {
            let start = para.take().unwrap_or(pos);
            flush_para(&mut out, src, Some(start), pos + q);
            out.push(block);
            pos = next;
            if src.as_bytes().get(pos) == Some(&b'\n') {
                pos += 1;
                mid = false;
            } else {
                mid = true;
            }
            continue;
        }
        if para.is_none() {
            para = Some(pos);
        }
        pos = end + 1;
        mid = false;
    }
    flush_para(&mut out, src, para, len);
    out
}

fn flush_para(out: &mut Vec<Block>, src: &str, start: Option<usize>, end: usize) {
    if let Some(s) = start
        && s < end
    {
        let text = src[s..end].trim();
        if !text.is_empty() {
            out.push(Block::Paragraph(parse_inline(text)));
        }
    }
}

fn split_indent(line: &str) -> (usize, &str) {
    let body = line.trim_start_matches([' ', '\t']);
    let cols = line[..line.len() - body.len()]
        .bytes()
        .map(|c| if c == b'\t' { 4 } else { 1 })
        .sum();
    (cols, body)
}

fn line_end(src: &str, pos: usize) -> usize {
    src[pos..].find('\n').map_or(src.len(), |k| pos + k)
}

/// Tries to start a line-level block at `pos`; returns the position after it.
fn block_at(
    src: &str,
    pos: usize,
    end: usize,
    in_quote: bool,
    out: &mut Vec<Block>,
) -> Option<usize> {
    let line = &src[pos..end];
    let (_, body) = split_indent(line);
    let first = *body.as_bytes().first()?;
    match first {
        b'>' if !in_quote => {
            if let Some(after) = body.strip_prefix(">>>") {
                let rest = after.strip_prefix(' ')?;
                let start = end - rest.len();
                let inner = &src[start..];
                if inner.trim().is_empty() {
                    return None;
                }
                out.push(Block::Quote(parse_blocks(inner, true)));
                return Some(src.len());
            }
            body.starts_with("> ").then(|| quote_lines(src, pos, out))
        }
        b'#' => {
            let hashes = body.bytes().take_while(|&c| c == b'#').count();
            let content = body[hashes..].strip_prefix(' ')?.trim();
            if !(1..=3).contains(&hashes) || content.is_empty() {
                return None;
            }
            out.push(Block::Heading {
                level: hashes as u8,
                content: parse_inline(content),
            });
            Some(end + 1)
        }
        b'-' | b'*' | b'0'..=b'9' => {
            if let Some(content) = body.strip_prefix("-# ") {
                let content = content.trim();
                if content.is_empty() {
                    return None;
                }
                out.push(Block::Subtext(parse_inline(content)));
                return Some(end + 1);
            }
            list_marker(body)?;
            Some(list_lines(src, pos, out))
        }
        _ => None,
    }
}

fn quote_lines(src: &str, pos: usize, out: &mut Vec<Block>) -> usize {
    let mut buf = String::new();
    let mut p = pos;
    while p < src.len() {
        let e = line_end(src, p);
        let (_, body) = split_indent(&src[p..e]);
        let Some(content) = body.strip_prefix("> ") else {
            break;
        };
        if p != pos {
            buf.push('\n');
        }
        buf.push_str(content);
        p = e + 1;
    }
    let inner = parse_blocks(&buf, true);
    if !inner.is_empty() {
        out.push(Block::Quote(inner));
    }
    p.min(src.len())
}

struct Item<'a> {
    indent: usize,
    ordered: bool,
    num: u32,
    text: &'a str,
}

/// `- x`, `* x` or `12. x`; returns (ordered, number, content).
fn list_marker(body: &str) -> Option<(bool, u32, &str)> {
    let b = body.as_bytes();
    let (ordered, num, rest) = match *b.first()? {
        b'-' | b'*' => (false, 0, body[1..].strip_prefix(' ')?),
        b'0'..=b'9' => {
            let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
            if digits > 9 {
                return None;
            }
            let rest = body[digits..].strip_prefix('.')?.strip_prefix(' ')?;
            (true, body[..digits].parse().ok()?, rest)
        }
        _ => return None,
    };
    let text = rest.trim();
    (!text.is_empty()).then_some((ordered, num, text))
}

fn list_lines(src: &str, pos: usize, out: &mut Vec<Block>) -> usize {
    let mut items = Vec::new();
    let mut p = pos;
    while p < src.len() {
        let e = line_end(src, p);
        let (indent, body) = split_indent(&src[p..e]);
        let Some((ordered, num, text)) = list_marker(body) else {
            break;
        };
        items.push(Item {
            indent,
            ordered,
            num,
            text,
        });
        p = e + 1;
    }
    let mut i = 0;
    while i < items.len() {
        let indent = items[i].indent;
        out.push(build_list(&items, &mut i, indent, 0));
    }
    p.min(src.len())
}

/// Consumes items starting at `*pos` (which sits at `indent`) into one list; deeper items
/// become nested lists of the preceding item.
fn build_list(items: &[Item], pos: &mut usize, indent: usize, depth: usize) -> Block {
    let first = &items[*pos];
    let (ordered, start) = (first.ordered, first.num);
    let mut out: Vec<Vec<Block>> = Vec::new();
    while *pos < items.len() {
        let it = &items[*pos];
        if it.indent < indent {
            break;
        }
        if it.indent > indent && depth < MAX_LIST_DEPTH {
            if let Some(last) = out.last_mut() {
                let sub = build_list(items, pos, it.indent, depth + 1);
                last.push(sub);
                continue;
            }
        } else if it.indent == indent && it.ordered != ordered {
            break;
        }
        out.push(vec![Block::Paragraph(parse_inline(it.text))]);
        *pos += 1;
    }
    Block::List {
        ordered,
        start,
        items: out,
    }
}

/// Finds a terminated ``` fence opening on this line; returns (offset in line, block, resume position).
fn fence_in_line(
    src: &str,
    pos: usize,
    end: usize,
    last_fence: Option<usize>,
) -> Option<(usize, Block, usize)> {
    let mut from = pos;
    while let Some(k) = src[from..end].find("```") {
        let p = from + k;
        if last_fence.is_none_or(|l| l < p + 3) {
            return None;
        }
        let close = p + 3 + src[p + 3..].find("```")?;
        if let Some(block) = code_block(&src[p + 3..close]) {
            return Some((p - pos, block, close + 3));
        }
        from = p + 3;
    }
    None
}

fn code_block(inner: &str) -> Option<Block> {
    let (lang, code) = match inner.find('\n') {
        Some(k) if is_lang(&inner[..k]) => (Some(inner[..k].to_string()), &inner[k + 1..]),
        _ => (None, inner),
    };
    let code = code.trim_start_matches('\n').trim_end_matches('\n');
    if lang.is_none() && code.trim().is_empty() {
        return None;
    }
    Some(Block::CodeBlock {
        lang,
        code: code.to_string(),
    })
}
