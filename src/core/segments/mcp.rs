use super::{Segment, SegmentData};
use crate::config::{InputData, SegmentId, StyleMode};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct McpSegment;

impl McpSegment {
    pub fn new() -> Self {
        Self
    }
}

impl Segment for McpSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let names = enabled_mcp_servers(&input.workspace.current_dir);
        if names.is_empty() {
            return None;
        }
        let mut metadata = HashMap::new();
        // Unit separator (U+001F) as delimiter — safe since server names are ASCII
        metadata.insert("mcp_servers".to_string(), names.join("\u{1f}"));
        Some(SegmentData {
            primary: names.join(", "),
            secondary: String::new(),
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::Mcp
    }
}

/// Collect names of all *enabled* (and not explicitly disabled) MCP servers from Claude's config.
/// Sources (best-effort, all failures silently ignored):
///   1. ~/.claude.json  — user-level and local-project mcpServers
///   2. <cwd>/.mcp.json — project-scoped servers (gated by approve/deny lists)
///   3. ~/.claude/settings.json enabledPlugins → each plugin's bundled .mcp.json
///
/// Note: connection/"failing" state is not available from these portable config files
/// (it would require the non-standard ~/.claude/mcp-health-cache.json), so only the
/// explicit disabledMcpServers list is used to filter.
pub fn enabled_mcp_servers(cwd: &str) -> Vec<String> {
    let mut servers: HashSet<String> = HashSet::new();
    let home = dirs::home_dir();

    // Servers explicitly disabled for this project (bare names and "plugin:<name>:<key>" forms)
    let mut project_disabled: HashSet<String> = HashSet::new();

    // 1 + 2: ~/.claude.json provides user/local servers and .mcp.json approval gates
    'claude_json: {
        let Some(ref home) = home else {
            break 'claude_json;
        };
        let Ok(content) = std::fs::read_to_string(home.join(".claude.json")) else {
            break 'claude_json;
        };
        let Ok(root) = serde_json::from_str::<serde_json::Value>(&content) else {
            break 'claude_json;
        };

        let proj = root.get("projects").and_then(|p| p.get(cwd));

        // Populate disabled set before any insertions so we can filter at insert time
        if let Some(arr) = proj
            .and_then(|p| p.get("disabledMcpServers"))
            .and_then(|v| v.as_array())
        {
            project_disabled.extend(arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())));
        }

        // User-level mcpServers (always enabled unless explicitly disabled)
        if let Some(obj) = root.get("mcpServers").and_then(|v| v.as_object()) {
            for key in obj.keys() {
                if !project_disabled.contains(key) {
                    servers.insert(key.clone());
                }
            }
        }

        // Local project mcpServers (always enabled unless explicitly disabled)
        if let Some(obj) = proj
            .and_then(|p| p.get("mcpServers"))
            .and_then(|v| v.as_object())
        {
            for key in obj.keys() {
                if !project_disabled.contains(key) {
                    servers.insert(key.clone());
                }
            }
        }

        // Approval gates for .mcp.json servers
        let enable_all = proj
            .and_then(|p| p.get("enableAllProjectMcpServers"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let enabled_set: HashSet<String> = proj
            .and_then(|p| p.get("enabledMcpjsonServers"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let disabled_set: HashSet<String> = proj
            .and_then(|p| p.get("disabledMcpjsonServers"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // 2. cwd/.mcp.json — project-scoped, gated by above sets and disabled list
        if let Ok(c2) = std::fs::read_to_string(std::path::Path::new(cwd).join(".mcp.json")) {
            if let Ok(v2) = serde_json::from_str::<serde_json::Value>(&c2) {
                if let Some(obj) = v2.get("mcpServers").and_then(|v| v.as_object()) {
                    for key in obj.keys() {
                        if !disabled_set.contains(key)
                            && (enable_all || enabled_set.contains(key))
                            && !project_disabled.contains(key)
                        {
                            servers.insert(key.clone());
                        }
                    }
                }
            }
        }
    }

    // 3. Enabled plugins from ~/.claude/settings.json
    'plugins: {
        let Some(ref home) = home else {
            break 'plugins;
        };
        let Ok(content) = std::fs::read_to_string(home.join(".claude").join("settings.json"))
        else {
            break 'plugins;
        };
        let Ok(sv) = serde_json::from_str::<serde_json::Value>(&content) else {
            break 'plugins;
        };
        let Some(plugins) = sv.get("enabledPlugins").and_then(|v| v.as_object()) else {
            break 'plugins;
        };

        for (spec, enabled) in plugins {
            if !enabled.as_bool().unwrap_or(false) {
                continue;
            }
            let mut parts = spec.splitn(2, '@');
            let (Some(name), Some(marketplace)) = (parts.next(), parts.next()) else {
                continue;
            };
            let plugin_dir = home
                .join(".claude")
                .join("plugins")
                .join("cache")
                .join(marketplace)
                .join(name);
            let Ok(rd) = std::fs::read_dir(&plugin_dir) else {
                continue;
            };
            for entry in rd.flatten() {
                let mcp_json = entry.path().join(".mcp.json");
                if let Ok(c3) = std::fs::read_to_string(&mcp_json) {
                    if let Ok(v3) = serde_json::from_str::<serde_json::Value>(&c3) {
                        if let Some(obj) = v3.get("mcpServers").and_then(|v| v.as_object()) {
                            for server_key in obj.keys() {
                                // Plugin servers are disabled via "plugin:<name>:<key>" form
                                let plugin_id = format!("plugin:{}:{}", name, server_key);
                                if !project_disabled.contains(&plugin_id) {
                                    servers.insert(server_key.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let mut names: Vec<String> = servers.into_iter().collect();
    names.sort();
    names
}

// ── Render ───────────────────────────────────────────────────────────────────

const RED: (u8, u8, u8) = (229, 57, 53);
const GREEN: (u8, u8, u8) = (67, 181, 96);
const ORANGE: (u8, u8, u8) = (255, 122, 41);
const AQUA: (u8, u8, u8) = (32, 201, 184);
const GRAY: (u8, u8, u8) = (138, 138, 138);

fn lighten(c: (u8, u8, u8)) -> (u8, u8, u8) {
    let blend = |v: u8| v.saturating_add(((255u16 - v as u16) * 3 / 10) as u8);
    (blend(c.0), blend(c.1), blend(c.2))
}

/// Reduce saturation to ~30% of original (HSL S × 0.3), preserving hue and lightness.
fn desaturate(c: (u8, u8, u8)) -> (u8, u8, u8) {
    let r = c.0 as f32 / 255.0;
    let g = c.1 as f32 / 255.0;
    let b = c.2 as f32 / 255.0;

    let cmax = r.max(g).max(b);
    let cmin = r.min(g).min(b);
    let delta = cmax - cmin;

    let l = (cmax + cmin) / 2.0;
    let s = if delta == 0.0 {
        0.0
    } else {
        delta / (1.0 - (2.0 * l - 1.0).abs())
    };
    let h = if delta == 0.0 {
        0.0_f32
    } else if cmax == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if cmax == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };

    let s2 = s * 0.30;
    let c2 = (1.0 - (2.0 * l - 1.0).abs()) * s2;
    let x = c2 * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c2 / 2.0;
    let (r1, g1, b1) = if h < 60.0 {
        (c2, x, 0.0)
    } else if h < 120.0 {
        (x, c2, 0.0)
    } else if h < 180.0 {
        (0.0, c2, x)
    } else if h < 240.0 {
        (0.0, x, c2)
    } else if h < 300.0 {
        (x, 0.0, c2)
    } else {
        (c2, 0.0, x)
    };
    let to_u8 = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    (to_u8(r1), to_u8(g1), to_u8(b1))
}

fn palette1(parent: (u8, u8, u8)) -> [(u8, u8, u8); 2] {
    if parent == RED || parent == ORANGE {
        [RED, ORANGE]
    } else {
        [GREEN, AQUA]
    }
}

fn rgb(text: &str, color: (u8, u8, u8)) -> String {
    format!(
        "\x1b[38;2;{};{};{}m{}\x1b[0m",
        color.0, color.1, color.2, text
    )
}

/// Strip ANSI escapes and return terminal cell width (emoji = 2 cells).
fn display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    let mut visible = String::new();
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            visible.push(ch);
            continue;
        }
        match chars.peek() {
            Some(&'[') => {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_alphabetic() {
                        break;
                    }
                }
            }
            Some(&']') => {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    UnicodeWidthStr::width(visible.as_str())
}

/// Render a sorted list of MCP server names as a compact, optionally colored string.
/// In NerdFont mode: brand tokens → glyphs, env suffixes → emoji, then brace-compress, then color.
/// In Plain mode: brace-compress the raw names (no glyphs, no colors).
///
/// Note: the body uses its own hardcoded RGB palette (alternating red/green and sub-palettes)
/// rather than the segment's configured `text` color; only `icon` color is used externally.
pub fn render_servers(names: &[String], mode: StyleMode) -> String {
    render_servers_lines(names, mode, None, 0)
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// Like `render_servers` but packs root items into multiple lines when `width` is given.
///
/// - `width: None` → single line (same as `render_servers`).
/// - `width: Some(w)` → greedy line packing; line 0 uses `first_line_budget` cells,
///   continuation lines use `w` cells. Breaking only at root-item boundaries.
pub fn render_servers_lines(
    names: &[String],
    mode: StyleMode,
    width: Option<usize>,
    first_line_budget: usize,
) -> Vec<String> {
    if names.is_empty() {
        return vec![String::new()];
    }

    let mut substituted: Vec<String> = names
        .iter()
        .map(|n| {
            if mode != StyleMode::Plain {
                substitute_brands(n)
            } else {
                n.clone()
            }
        })
        .collect();
    // Brand/env substitution can change sort order (glyphs are high codepoints), so re-sort.
    // Dedup after sort: substitution is many-to-one (e.g. "postgres"/"postgresql" → same glyph),
    // so two distinct input names can collide; duplicates would break try_token_product's
    // Cartesian-product size check.
    substituted.sort();
    substituted.dedup();

    let tree = build_tree(&substituted);

    let sep = if mode == StyleMode::Plain {
        ", ".to_string()
    } else {
        format!("{} ", rgb(",", GRAY))
    };
    // ", " is always 2 visible cells regardless of ANSI wrapping
    const SEP_VIS: usize = 2;

    // Render each root item individually
    let items: Vec<(String, usize)> = tree
        .iter()
        .enumerate()
        .map(|(i, pattern)| {
            let color = if i.is_multiple_of(2) { RED } else { GREEN };
            let rendered = render_pattern(pattern, color, mode);
            let vis = display_width(&rendered);
            (rendered, vis)
        })
        .collect();

    match width {
        None => {
            // Single line — join all items with sep
            let parts: Vec<&str> = items.iter().map(|(s, _)| s.as_str()).collect();
            vec![parts.join(&sep)]
        }
        Some(total_width) => {
            // Greedy packing: break only between root items
            let mut lines: Vec<String> = Vec::new();
            let mut current_line = String::new();
            let mut current_vis: usize = 0;

            for (item, item_vis) in &items {
                let budget = if lines.is_empty() {
                    first_line_budget
                } else {
                    total_width
                };

                if current_vis > 0 && current_vis + SEP_VIS + item_vis > budget {
                    lines.push(std::mem::take(&mut current_line));
                    current_vis = 0;
                }

                if current_vis > 0 {
                    current_line.push_str(&sep);
                    current_vis += SEP_VIS;
                }
                current_line.push_str(item);
                current_vis += item_vis;
            }
            lines.push(current_line);
            lines
        }
    }
}

// ── Brand substitution ────────────────────────────────────────────────────────

// Map from token lowercase to NF glyph (exact token equality match, order is irrelevant)
const BRANDS: &[(&str, &str)] = &[
    ("postgresql", "\u{e76e}"),
    ("kubernetes", "\u{f10fe}"),
    ("prometheus", "\u{e870}"),
    ("grafana", "\u{e7f3}"),
    ("argocd", "\u{e734}"),
    ("pulumi", "\u{e873}"),
    ("github", "\u{f09b}"),
    ("notion", "\u{e848}"),
    ("sentry", "\u{e89f}"),
    ("vercel", "\u{e8d3}"),
    ("claude", "\u{2733}"),
    ("slack", "\u{f198}"),
    ("postgres", "\u{e76e}"),
    ("k8s", "\u{f10fe}"),
    ("aws", "\u{e7ad}"),
    ("git", "\u{e702}"),
];

// Environment-suffix tokens → representative emoji (trailing position only)
const ENVS: &[(&str, &str)] = &[("prod", "🚀"), ("staging", "🚧"), ("dev", "🔧")];

fn substitute_brands(name: &str) -> String {
    let mut result = String::new();
    let mut remaining = name;

    while !remaining.is_empty() {
        // Consume alphanumeric token
        let token_end = remaining
            .find(|c: char| !c.is_alphanumeric())
            .unwrap_or(remaining.len());
        let token = &remaining[..token_end];
        let token_lower = token.to_lowercase();
        remaining = &remaining[token_end..];

        // Is this the trailing token (nothing left after it)?
        let is_last = remaining.is_empty();

        // Trailing token: try env emoji substitution first
        if is_last {
            if let Some((_, emoji)) = ENVS.iter().find(|(e, _)| *e == token_lower.as_str()) {
                result.push_str(emoji);
                break;
            }
        }

        // Try brand substitution
        let brand = BRANDS
            .iter()
            .find(|(b, _)| *b == token_lower.as_str())
            .map(|(_, g)| *g)
            .unwrap_or(token);
        result.push_str(brand);

        if remaining.is_empty() {
            break;
        }

        // Consume delimiter (non-alphanumeric run)
        let delim_end = remaining
            .find(|c: char| c.is_alphanumeric())
            .unwrap_or(remaining.len());
        result.push_str(&remaining[..delim_end]);
        remaining = &remaining[delim_end..];
    }

    result
}

// ── Brace-compression tree ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Factor {
    Lit(String),
    Group(usize, Vec<Vec<Factor>>), // (level, alternatives)
}

type Pattern = Vec<Factor>;

/// Build the compression tree from a sorted, deduplicated list of strings.
fn build_tree(names: &[String]) -> Vec<Pattern> {
    // Bucket by first char, preserving order
    let mut buckets: Vec<Vec<String>> = Vec::new();
    let mut bucket_start = 0;

    let mut i = 1;
    while i <= names.len() {
        let end = i == names.len() || names[i].chars().next() != names[bucket_start].chars().next();
        if end {
            buckets.push(names[bucket_start..i].to_vec());
            bucket_start = i;
        }
        i += 1;
    }

    buckets
        .into_iter()
        .map(|bucket| factor_item(&bucket, 0))
        .collect()
}

/// Explicit delimiter characters for token-level product factoring.
/// NF glyphs and env emoji are NOT in this set so they remain field content.
const TOKEN_DELIMS: &[char] = &['-', '_', '.', ':', '/', ' '];

/// Try to factor a bucket as a Cartesian product of token-level fields separated by delimiters.
/// Returns `Some(pattern)` if the bucket is a clean product, `None` to fall through.
fn try_token_product(bucket: &[String], level: usize) -> Option<Pattern> {
    if bucket.len() <= 1 {
        return None;
    }

    // Tokenize each name: split on TOKEN_DELIMS, collect (fields, delimiters)
    let tokenized: Vec<(Vec<String>, Vec<char>)> = bucket
        .iter()
        .map(|name| {
            let mut fields = Vec::new();
            let mut delimiters = Vec::new();
            let mut field_start = 0;
            for (i, c) in name.char_indices() {
                if TOKEN_DELIMS.contains(&c) {
                    fields.push(name[field_start..i].to_string());
                    delimiters.push(c);
                    field_start = i + c.len_utf8();
                }
            }
            fields.push(name[field_start..].to_string());
            (fields, delimiters)
        })
        .collect();

    // Require at least one delimiter (≥ 2 fields); no-delimiter names stay on char-level path
    if tokenized[0].1.is_empty() {
        return None;
    }

    // All names must have the same delimiter sequence
    let first_delims = &tokenized[0].1;
    for (_, delims) in &tokenized[1..] {
        if delims != first_delims {
            return None;
        }
    }

    let n_fields = tokenized[0].0.len();

    // Build per-position value sets (sorted, unique)
    let per_pos: Vec<Vec<String>> = (0..n_fields)
        .map(|i| {
            let mut vals: Vec<String> = tokenized
                .iter()
                .map(|(fields, _)| fields[i].clone())
                .collect();
            vals.sort();
            vals.dedup();
            vals
        })
        .collect();

    // The product is valid iff ∏|Sᵢ| == bucket.len()
    // (every name's field at pos i is in per_pos[i] by construction, so the Cartesian
    // product exactly covers the bucket when the count matches)
    let product_size: usize = per_pos.iter().map(|v| v.len()).product();
    if product_size != bucket.len() {
        return None;
    }

    // Emit Pattern: Lit(field) or union_inner for multi-value positions, separated by Lit(delim)
    let mut pattern: Pattern = Vec::new();
    for (i, vals) in per_pos.iter().enumerate() {
        if i > 0 {
            pattern.push(Factor::Lit(first_delims[i - 1].to_string()));
        }
        if vals.len() == 1 {
            pattern.push(Factor::Lit(vals[0].clone()));
        } else {
            pattern.extend(union_inner(vals, level));
        }
    }
    Some(pattern)
}

/// Returns `(byte_pos_of_delim, delim_char, trailing_token)` for the last
/// TOKEN_DELIMS-separated token of `s`, or `None` if there is no delimiter or the
/// token after the last delimiter is empty.
fn trailing_suffix(s: &str) -> Option<(usize, char, String)> {
    let (pos, delim) = s
        .char_indices()
        .rfind(|&(_, c)| TOKEN_DELIMS.contains(&c))?;
    let token = s[pos + delim.len_utf8()..].to_string();
    if token.is_empty() {
        return None;
    }
    Some((pos, delim, token))
}

/// A stable sort key for an alternative pattern: concatenate literals, following
/// the first alternative in every Group. Used to alphabetically order outer alts.
fn alt_sort_key(pattern: &Pattern) -> String {
    let mut key = String::new();
    for factor in pattern {
        match factor {
            Factor::Lit(s) => key.push_str(s),
            Factor::Group(_, alts) => {
                if let Some(first) = alts.first() {
                    key.push_str(&alt_sort_key(first));
                }
            }
        }
    }
    key
}

/// Try to factor items that share common trailing tokens.
/// Called from `union_inner` after Cartesian-product check fails.
///
/// Groups items by `(delim, trailing_token)`, then clusters groups that share the
/// same stripped-prefix set. Each cluster with ≥ 2 tokens becomes a nested product
/// `{prefixes}-{tok1,tok2,…}`; clusters with 1 token stay as `{prefixes}-tok`.
/// Items with no trailing delimiter become individual Lit alts.
/// Returns `None` when no suffix group has ≥ 2 members (fall through to char-bucket path).
fn try_partial_suffix(unique: &[String], outer_level: usize) -> Option<Pattern> {
    use std::collections::HashMap;

    if unique.len() <= 1 {
        return None;
    }

    // Compute the trailing suffix for each item (byte_pos, delim, token).
    let trails: Vec<Option<(usize, char, String)>> =
        unique.iter().map(|s| trailing_suffix(s)).collect();

    // Group item indices by (delim_char, token).
    let mut suffix_groups: HashMap<(char, String), Vec<usize>> = HashMap::new();
    for (i, trail) in trails.iter().enumerate() {
        if let Some((_, delim, token)) = trail {
            suffix_groups
                .entry((*delim, token.clone()))
                .or_default()
                .push(i);
        }
    }

    // Only proceed if at least one suffix group has ≥ 2 members.
    if !suffix_groups.values().any(|v| v.len() >= 2) {
        return None;
    }

    // Cluster suffix groups by (delim, stripped-prefix-set).
    // Groups sharing the same delim and prefix set merge into one nested product.
    let mut clusters: HashMap<(char, Vec<String>), Vec<String>> = HashMap::new();
    for ((delim, token), indices) in &suffix_groups {
        let mut stripped: Vec<String> = indices
            .iter()
            .map(|&i| {
                let s = &unique[i];
                let pos = trails[i].as_ref().unwrap().0;
                s[..pos].to_string()
            })
            .collect();
        stripped.sort();
        stripped.dedup();
        clusters
            .entry((*delim, stripped))
            .or_default()
            .push(token.clone());
    }

    // Build one Pattern alt per cluster.
    let mut outer_alts: Vec<Pattern> = Vec::new();
    for ((delim, prefix_set), mut tokens) in clusters {
        tokens.sort();
        let mut alt = factor_item(&prefix_set, outer_level);
        alt.push(Factor::Lit(delim.to_string()));
        if tokens.len() == 1 {
            alt.push(Factor::Lit(tokens.remove(0)));
        } else {
            let token_alts: Vec<Pattern> =
                tokens.into_iter().map(|t| vec![Factor::Lit(t)]).collect();
            alt.push(Factor::Group(outer_level + 1, token_alts));
        }
        outer_alts.push(alt);
    }

    // Items with no trailing delimiter → individual Lit alts.
    for (i, trail) in trails.iter().enumerate() {
        if trail.is_none() {
            outer_alts.push(vec![Factor::Lit(unique[i].clone())]);
        }
    }

    // Sort alphabetically by each alt's first concrete string.
    outer_alts.sort_by_key(alt_sort_key);

    Some(vec![Factor::Group(outer_level, outer_alts)])
}

fn factor_item(bucket: &[String], level: usize) -> Pattern {
    if bucket.len() == 1 {
        return vec![Factor::Lit(bucket[0].clone())];
    }

    // Token-level product factoring: handles delimiter-separated names like
    // {postgres}-{control,keycloak,pub}-{🔧,🚧} before falling back to char-level LCP/LCS.
    if let Some(p) = try_token_product(bucket, level) {
        return p;
    }

    let p = lcp(bucket);
    let after_p: Vec<String> = bucket.iter().map(|s| chars_drop_prefix(s, &p)).collect();

    let q = lcs(&after_p);
    let middles: Vec<String> = after_p.iter().map(|s| chars_drop_suffix(s, &q)).collect();

    let mut result = Pattern::new();
    if !p.is_empty() {
        result.push(Factor::Lit(p));
    }
    result.extend(union_inner(&middles, level));
    if !q.is_empty() {
        result.push(Factor::Lit(q));
    }
    result
}

fn union_inner(m: &[String], level: usize) -> Pattern {
    // Deduplicate (bucket items should already be unique, but be defensive)
    let mut unique: Vec<String> = m.to_vec();
    unique.sort();
    unique.dedup();

    if unique.len() == 1 {
        return vec![Factor::Lit(unique[0].clone())];
    }

    let next_level = level + 1;

    if next_level > 2 {
        // Cap: flat group, no recursion
        let alts: Vec<Pattern> = unique
            .iter()
            .map(|s| vec![Factor::Lit(s.clone())])
            .collect();
        return vec![Factor::Group(next_level, alts)];
    }

    // Cartesian product check
    if let Some(char_sets) = is_cartesian_product(&unique) {
        // One Group per position
        return char_sets
            .into_iter()
            .map(|chars| {
                let alts: Vec<Pattern> = chars
                    .into_iter()
                    .map(|c| vec![Factor::Lit(c.to_string())])
                    .collect();
                Factor::Group(next_level, alts)
            })
            .collect();
    }

    // Partial suffix factoring: if a subset (≥ 2 items but not all) shares a
    // trailing token, compress it as {subset}-<token> alongside the leftovers.
    if let Some(p) = try_partial_suffix(&unique, next_level) {
        return p;
    }

    // Bucket by first char, recurse
    let mut sub_buckets: Vec<Vec<String>> = Vec::new();
    let mut start = 0;
    let mut i = 1;
    while i <= unique.len() {
        let end = i == unique.len() || unique[i].chars().next() != unique[start].chars().next();
        if end {
            sub_buckets.push(unique[start..i].to_vec());
            start = i;
        }
        i += 1;
    }

    let alts: Vec<Pattern> = sub_buckets
        .into_iter()
        .map(|b| factor_item(&b, next_level))
        .collect();

    vec![Factor::Group(next_level, alts)]
}

/// Returns `Some(vec_of_char_sets_per_position)` iff `m` is the full Cartesian product
/// of equal-length strings. Each char set is sorted and deduplicated.
fn is_cartesian_product(m: &[String]) -> Option<Vec<Vec<char>>> {
    if m.len() <= 1 {
        return None;
    }
    let len = m[0].chars().count();
    if len == 0 || !m.iter().all(|s| s.chars().count() == len) {
        return None;
    }

    let char_sets: Vec<Vec<char>> = (0..len)
        .map(|i| {
            let mut chars: Vec<char> = m.iter().map(|s| s.chars().nth(i).unwrap()).collect();
            chars.sort();
            chars.dedup();
            chars
        })
        .collect();

    let product_size: usize = char_sets.iter().map(|c| c.len()).product();
    if product_size != m.len() {
        return None;
    }

    // Every string must have each char at position i from char_sets[i]
    if m.iter().all(|s| {
        s.chars()
            .enumerate()
            .all(|(i, c)| char_sets[i].contains(&c))
    }) {
        Some(char_sets)
    } else {
        None
    }
}

// ── String helpers (char-aware) ───────────────────────────────────────────────

fn lcp(strings: &[String]) -> String {
    if strings.is_empty() {
        return String::new();
    }
    let first: Vec<char> = strings[0].chars().collect();
    let mut len = first.len();
    for s in &strings[1..] {
        let sc: Vec<char> = s.chars().collect();
        let common = first[..len]
            .iter()
            .zip(sc.iter())
            .take_while(|(a, b)| a == b)
            .count();
        len = common;
        if len == 0 {
            break;
        }
    }
    first[..len].iter().collect()
}

fn lcs(strings: &[String]) -> String {
    if strings.is_empty() {
        return String::new();
    }
    let first: Vec<char> = strings[0].chars().collect();
    let mut len = first.len();
    for s in &strings[1..] {
        let sc: Vec<char> = s.chars().collect();
        let capped = len.min(sc.len());
        let common = first[first.len() - capped..]
            .iter()
            .rev()
            .zip(sc[sc.len() - capped..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        len = common;
        if len == 0 {
            break;
        }
    }
    let fc: Vec<char> = strings[0].chars().collect();
    fc[fc.len() - len..].iter().collect()
}

fn chars_drop_prefix(s: &str, prefix: &str) -> String {
    let plen = prefix.chars().count();
    s.chars().skip(plen).collect()
}

fn chars_drop_suffix(s: &str, suffix: &str) -> String {
    let slen = suffix.chars().count();
    let chars: Vec<char> = s.chars().collect();
    chars[..chars.len().saturating_sub(slen)].iter().collect()
}

// ── Tree rendering ────────────────────────────────────────────────────────────

fn render_pattern(pattern: &Pattern, color: (u8, u8, u8), mode: StyleMode) -> String {
    pattern
        .iter()
        .map(|f| render_factor(f, color, mode))
        .collect()
}

fn render_factor(factor: &Factor, color: (u8, u8, u8), mode: StyleMode) -> String {
    match factor {
        Factor::Lit(s) => {
            if mode == StyleMode::Plain {
                s.clone()
            } else {
                rgb(s, color)
            }
        }
        Factor::Group(level, alts) => {
            if mode == StyleMode::Plain {
                let inner: Vec<String> = alts
                    .iter()
                    .map(|a| render_pattern(a, color, mode))
                    .collect();
                format!("{{{}}}", inner.join(","))
            } else {
                let mut out = rgb("{", GRAY);
                for (j, alt) in alts.iter().enumerate() {
                    if j > 0 {
                        out.push_str(&rgb(",", GRAY));
                    }
                    let alt_color = alt_color(*level, j, color);
                    out.push_str(&render_pattern(alt, alt_color, mode));
                }
                out.push_str(&rgb("}", GRAY));
                out
            }
        }
    }
}

fn alt_color(level: usize, j: usize, parent: (u8, u8, u8)) -> (u8, u8, u8) {
    match level {
        1 => palette1(parent)[j % 2],
        2 => {
            if j.is_multiple_of(2) {
                parent
            } else {
                lighten(parent)
            }
        }
        _ => {
            if j.is_multiple_of(2) {
                parent
            } else {
                desaturate(parent)
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn compress(names: &[&str]) -> String {
        let v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        render_servers(&v, StyleMode::Plain)
    }

    #[test]
    fn test_simple_prefix() {
        assert_eq!(compress(&["ab", "ac", "ad", "ba"]), "a{b,c,d}, ba");
    }

    #[test]
    fn test_cartesian() {
        assert_eq!(compress(&["aaa", "aab", "aba", "abb"]), "a{a,b}{a,b}");
    }

    #[test]
    fn test_brand_github_split() {
        let subst: Vec<String> = ["github-1", "github-2"]
            .iter()
            .map(|n| substitute_brands(n))
            .collect();
        // Both start with the github glyph
        assert!(subst[0].starts_with('\u{f09b}'));
        assert!(subst[1].starts_with('\u{f09b}'));
        // After compression the glyph prefix is shared
        let result = compress(&[&subst[0], &subst[1]]);
        // Should be "{glyph}-{1,2}"
        assert!(result.contains("{1,2}"), "got: {}", result);
    }

    #[test]
    fn test_brand_github_gitlab() {
        let subst: Vec<String> = ["github", "gitlab"]
            .iter()
            .map(|n| substitute_brands(n))
            .collect();
        // github → glyph, gitlab stays
        assert_eq!(subst[0], "\u{f09b}");
        assert_eq!(subst[1], "gitlab");
    }

    #[test]
    fn test_brand_substitution_standalone() {
        assert_eq!(substitute_brands("aws"), "\u{e7ad}");
        assert_eq!(substitute_brands("claude"), "\u{2733}");
        assert_eq!(substitute_brands("k8s"), "\u{f10fe}");
        assert_eq!(substitute_brands("postgres"), "\u{e76e}");
        assert_eq!(substitute_brands("postgresql"), "\u{e76e}");
    }

    #[test]
    fn test_no_brand_substitution_in_plain() {
        let result = render_servers(&["github".to_string()], StyleMode::Plain);
        assert_eq!(result, "github");
    }

    #[test]
    fn test_token_product_postgres() {
        // Clean product: postgres-{control,keycloak,pub}-{dev,staging} in Plain mode
        assert_eq!(
            compress(&[
                "postgres-control-dev",
                "postgres-control-staging",
                "postgres-keycloak-dev",
                "postgres-keycloak-staging",
                "postgres-pub-dev",
                "postgres-pub-staging",
            ]),
            "postgres-{control,keycloak,pub}-{dev,staging}"
        );
    }

    #[test]
    fn test_token_product_does_not_break_existing() {
        // Names without delimiters must stay on the char-level path
        assert_eq!(compress(&["ab", "ac", "ad", "ba"]), "a{b,c,d}, ba");
        assert_eq!(compress(&["aaa", "aab", "aba", "abb"]), "a{a,b}{a,b}");
    }

    #[test]
    fn test_env_substitution_trailing() {
        // Trailing token gets emoji; middle token does not
        assert_eq!(
            substitute_brands("postgres-control-dev"),
            "\u{e76e}-control-🔧"
        );
        assert_eq!(substitute_brands("postgres-pub-staging"), "\u{e76e}-pub-🚧");
        assert_eq!(substitute_brands("service-prod"), "service-🚀");
        // "dev" in middle position → no env emoji (brand sub only, no match → kept as-is)
        assert_eq!(substitute_brands("dev-postgres"), "dev-\u{e76e}");
    }

    #[test]
    fn test_render_servers_lines_wrap() {
        // With a narrow first-line budget, items break only at root boundaries.
        // Items: "aaa","bbb","ccc","ddd" each 3 chars; sep = 2 chars.
        // first_line_budget=3: only "aaa" fits on line 0.
        // total_width=10: "bbb, ccc"=8 fits, adding ", ddd"=5 → 13 > 10 → break.
        // Expected: ["aaa", "bbb, ccc", "ddd"]
        let names: Vec<String> = ["aaa", "bbb", "ccc", "ddd"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let lines = render_servers_lines(&names, StyleMode::Plain, Some(10), 3);
        assert_eq!(lines, vec!["aaa", "bbb, ccc", "ddd"]);
    }

    #[test]
    fn test_partial_suffix_aws_docs() {
        // 7 of 8 share -dev; docs has no env suffix.
        // cloud{trail,watch} come from the LCP path inside the stripped sub-group.
        assert_eq!(
            compress(&[
                "aws-api-dev",
                "aws-billing-dev",
                "aws-cloudtrail-dev",
                "aws-cloudwatch-dev",
                "aws-docs",
                "aws-eks-dev",
                "aws-iam-dev",
                "aws-pricing-dev",
            ]),
            "aws-{{api,billing,cloud{trail,watch},eks,iam,pricing}-dev,docs}"
        );
    }

    #[test]
    fn test_partial_suffix_multi_token_recurses_on_leftovers() {
        // Two distinct trailing tokens → two sibling sub-groups + a singleton at the same level.
        // Items share prefix "svc-" so they enter union_inner together after LCP stripping.
        assert_eq!(
            compress(&[
                "svc-a-dev",
                "svc-b-dev",
                "svc-c-prod",
                "svc-d-prod",
                "svc-docs",
            ]),
            "svc-{{a,b}-dev,{c,d}-prod,docs}"
        );
    }

    #[test]
    fn test_clean_product_still_preferred_over_partial() {
        // try_token_product wins for clean products; partial factoring is unreachable here.
        assert_eq!(
            compress(&[
                "postgres-control-dev",
                "postgres-control-staging",
                "postgres-keycloak-dev",
                "postgres-keycloak-staging",
                "postgres-pub-dev",
                "postgres-pub-staging",
            ]),
            "postgres-{control,keycloak,pub}-{dev,staging}"
        );
    }

    #[test]
    fn test_partial_suffix_skipped_when_no_eligible_subset() {
        // All items have distinct first chars; no trailing-token group ≥ 2 → standard path.
        assert_eq!(compress(&["ab", "ac", "ad", "ba"]), "a{b,c,d}, ba");
    }

    #[test]
    fn test_partial_suffix_shared_prefix_multi_suffix() {
        // When multiple env suffixes share the same stripped-prefix set, they must collapse
        // into a nested product {prefixes}-{suffixes} rather than repeating the prefix set.
        assert_eq!(
            compress(&[
                "x-api-dev",
                "x-api-prod",
                "x-api-staging",
                "x-billing-dev",
                "x-billing-prod",
                "x-billing-staging",
                "x-docs",
                "x-eks-dev",
                "x-eks-prod",
                "x-eks-staging",
                "x-knowledge",
                "x-pricing-dev",
                "x-pricing-prod",
                "x-pricing-staging",
            ]),
            "x-{{api,billing,eks,pricing}-{dev,prod,staging},docs,knowledge}"
        );
    }

    #[test]
    fn test_desaturate_drops_saturation() {
        let (r, g, b) = desaturate(RED);
        // Desaturated red is a muted brownish-pink: not bright, and closer to gray.
        assert!(r > 100 && r < 210, "r={r}");
        assert!((r as i32 - g as i32).abs() < 70, "r={r} g={g}");
        assert!((r as i32 - b as i32).abs() < 70, "r={r} b={b}");
    }
}
