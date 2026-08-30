/// Right-hand side of `export const <name> = …`, taken from the real source.
///
/// Two things have to be true at once, and doing only one of them is how every
/// route-export scanner here used to get the answer wrong.
///
/// *The statement must be code.* Positions are found in `masked` — the shared
/// [`code_without_strings_and_comments`] view, where comment and literal text is
/// blanked but every byte offset still names the same place in `source`. Reading
/// the raw text instead made a commented-out `export const hydrate = false`, or
/// one quoted inside a documentation snippet, switch off the real page's
/// hydration.
///
/// *The value must be the source's.* Masking blanks string contents, so
/// `'idle'` reads as `'    '`. The span is located in `masked` and then sliced
/// out of `source`, which is what lets a string-valued export be recognised at
/// all.
///
/// A TypeScript annotation between the name and `=` is skipped. `has_export_function`
/// already tolerated one; these did not, so `export const revalidate: number = 3600`
/// silently lost its ISR opt-in and `export const ppr: boolean = true` its PPR opt-in.
///
/// Returns `None` when the declaration does not finish on its own line — the
/// scan is line-based and will not guess at a continuation.
pub(crate) fn export_const_value<'a>(source: &'a str, masked: &str, name: &str) -> Option<&'a str> {
    debug_assert_eq!(
        source.len(),
        masked.len(),
        "masked_code preserves length, which is what makes these offsets shared"
    );
    let prefix = format!("export const {name}");
    let mut line_start = 0usize;

    for line in masked.lines() {
        let start = line_start;
        // `masked_code` keeps every `\n` in place and turns a `\r` into a space,
        // so one byte always separates consecutive lines in both strings.
        line_start += line.len() + 1;

        let indent = line.len() - line.trim_start().len();
        let Some(after) = line[indent..].strip_prefix(prefix.as_str()) else {
            continue;
        };
        // `export const hydrateAll` is a different export.
        if after
            .chars()
            .next()
            .is_some_and(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
        {
            continue;
        }
        let Some(equals) = assignment_offset(after) else {
            continue;
        };

        let value_start = indent + prefix.len() + equals + 1;
        let masked_tail = &line[value_start..];
        let raw_tail = &source[start + value_start..start + line.len()];

        // Where the value ends depends on what masking left behind. When any
        // code survives — `false`, `3600`, `'idle' as const` — the last
        // non-blank byte is the end, and a trailing comment is already blank.
        // When nothing survives the value is one string literal, whose own text
        // was blanked; only then is the literal measured directly, over a span
        // masking has already proven holds no code.
        let end = if masked_tail.trim().is_empty() {
            quoted_literal_end(raw_tail)?
        } else {
            masked_tail.trim_end().len()
        };
        let value = raw_tail.get(..end)?.trim();
        return (!value.is_empty()).then_some(value);
    }
    None
}

/// Byte just past the quoted literal that starts `tail`, if one does.
///
/// Only reached for a value masking reported as entirely non-code, so this
/// measures one literal rather than lexing a program — there is no second
/// scanner here to drift from [`code_without_strings_and_comments`].
pub(crate) fn quoted_literal_end(tail: &str) -> Option<usize> {
    let bytes = tail.as_bytes();
    let start = bytes.iter().position(|byte| !byte.is_ascii_whitespace())?;
    let quote = bytes[start];
    if !matches!(quote, b'\'' | b'"' | b'`') {
        return None;
    }
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            byte if byte == quote => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

/// Offset of the assignment `=` in `after`, skipping any type annotation.
///
/// `=` also appears in `=>`, `==`, `<=`, `>=`, and `!=`, all of which occur
/// inside a type (`: Record<string, () => void>`), so only a bare `=` outside
/// every bracket pair counts.
///
/// `<` and `>` are deliberately not counted as a pair. They are not reliably
/// balanced in TypeScript — `=>` alone would close a depth nothing opened, and
/// comparison operators do the same — so tracking them turned an ordinary
/// annotation into a negative depth and lost the assignment entirely. The three
/// bracket pairs that are always balanced are enough to keep a `=` inside a
/// parameter list or object type from being mistaken for the assignment.
pub(crate) fn assignment_offset(after: &str) -> Option<usize> {
    let bytes = after.as_bytes();
    let mut depth = 0i32;
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'=' if depth == 0 => {
                let follows = bytes.get(index + 1);
                let precedes = index.checked_sub(1).map(|previous| bytes[previous]);
                if follows == Some(&b'=')
                    || follows == Some(&b'>')
                    || matches!(precedes, Some(b'=' | b'!' | b'<' | b'>'))
                {
                    continue;
                }
                return Some(index);
            }
            _ => {}
        }
    }
    None
}

/// Check if `export const <name> = true|false` exists.
pub(crate) fn has_export_const_bool(
    source: &str,
    masked: &str,
    name: &str,
    expected: bool,
) -> bool {
    export_const_value(source, masked, name)
        .map(|value| value.trim_end_matches(';').trim())
        .is_some_and(|value| value == if expected { "true" } else { "false" })
}

/// Parse `export const <name> = <number>` and return the number.
pub(crate) fn parse_export_const_number(source: &str, masked: &str, name: &str) -> Option<u64> {
    export_const_value(source, masked, name)?
        .trim_end_matches(';')
        .trim()
        .parse::<u64>()
        .ok()
}

/// Check if `export function <name>` or `export async function <name>` exists.
pub(crate) fn has_export_function(code: &str, name: &str) -> bool {
    let patterns = [
        format!("export function {name}"),
        format!("export async function {name}"),
        format!("export const {name}"),
    ];
    for line in code.lines() {
        let trimmed = line.trim();
        for pattern in &patterns {
            let Some(rest) = trimmed.strip_prefix(pattern.as_str()) else {
                continue;
            };
            if rest.chars().next().is_none_or(|character| {
                character.is_whitespace() || matches!(character, '(' | '<' | ':' | '=')
            }) {
                return true;
            }
        }
    }
    false
}

/// Names a page may use to declare its static parameter set.
///
/// `generateStaticParams` is Next.js's name for the same export, with the same
/// contract: return the parameter objects to pre-render. Accepting it costs
/// nothing and removes a silent failure — a page brought over from Next.js
/// declared its parameters, this file did not recognise the name, and the route
/// was served dynamically with no diagnostic anywhere. Mirrored by the resolver
/// in `packages/ruvyxa/runtime/worker-pool.mjs`, which has to read the same
/// names or discovery and execution disagree.
pub const STATIC_PARAMS_EXPORTS: [&str; 3] =
    ["getStaticParams", "staticParams", "generateStaticParams"];

pub(crate) fn has_static_params_export(code: &str) -> bool {
    STATIC_PARAMS_EXPORTS
        .iter()
        .any(|name| has_export_function(code, name))
}
