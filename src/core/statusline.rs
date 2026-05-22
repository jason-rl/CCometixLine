use crate::config::{AnsiColor, Config, SegmentConfig, StyleMode};
use crate::core::segments::SegmentData;

/// Strip ANSI escape sequences and return visible text length (in chars, not cells).
fn visible_width(text: &str) -> usize {
    let mut visible = String::new();
    let mut in_escape = false;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Start of ANSI escape sequence
            in_escape = true;
            // Skip the [ character
            if chars.peek() == Some(&'[') {
                chars.next();
            }
        } else if in_escape {
            // Skip until we find the end of the escape sequence (letter)
            if ch.is_alphabetic() {
                in_escape = false;
            }
        } else {
            // Regular character
            visible.push(ch);
        }
    }

    visible.chars().count()
}

/// Strip ANSI escape sequences and return terminal cell width (emoji = 2 cells).
fn display_width(text: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    let mut visible = String::new();
    let mut in_escape = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            in_escape = true;
            if chars.peek() == Some(&'[') {
                chars.next();
            }
        } else if in_escape {
            if ch.is_alphabetic() {
                in_escape = false;
            }
        } else {
            visible.push(ch);
        }
    }
    UnicodeWidthStr::width(visible.as_str())
}

pub struct StatusLineGenerator {
    config: Config,
}

impl StatusLineGenerator {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn generate(&self, segments: Vec<(SegmentConfig, SegmentData)>) -> String {
        use crate::config::SegmentId;
        use crate::core::segments::mcp;

        // Partition: Mcp goes on its own line; everything else on line 1
        let mut mcp_entry: Option<(SegmentConfig, SegmentData)> = None;
        let mut enabled_segments: Vec<(SegmentConfig, SegmentData)> = Vec::new();

        for (config, data) in segments {
            if !config.enabled {
                continue;
            }
            if config.id == SegmentId::Mcp {
                mcp_entry = Some((config, data));
            } else {
                enabled_segments.push((config, data));
            }
        }

        let mut output = Vec::new();
        for (config, data) in enabled_segments.iter() {
            let rendered = self.render_segment(config, data);
            if !rendered.is_empty() {
                output.push(rendered);
            }
        }

        let line1 = if output.is_empty() {
            String::new()
        } else if self.config.style.separator == "\u{e0b0}" {
            self.join_with_powerline_arrows(&output, &enabled_segments)
        } else {
            self.join_with_white_separators(&output)
        };

        // Right-justify line 1 when configured
        let line1 = if self.config.style.right_aligned {
            if let Ok((cols, _)) = crossterm::terminal::size() {
                let cols = cols as usize;
                let w = display_width(&line1);
                let pad = cols.saturating_sub(w);
                if pad > 0 {
                    format!("{}{}", " ".repeat(pad), line1)
                } else {
                    line1
                }
            } else {
                line1
            }
        } else {
            line1
        };

        // Build optional MCP second line
        if let Some((mcp_cfg, mcp_data)) = mcp_entry {
            if let Some(raw) = mcp_data.metadata.get("mcp_servers") {
                let names: Vec<String> = raw.split('\u{1f}').map(|s| s.to_string()).collect();

                // Plug glyph conforms to powerline styling on line 1.
                // Background borrowed from the leading line-1 segment so it auto-matches
                // the active theme without any config edits.
                let plug_bg = mcp_cfg.colors.background.clone().or_else(|| {
                    enabled_segments
                        .first()
                        .and_then(|(c, _)| c.colors.background.clone())
                });

                let icon_glyph = self.get_icon(&mcp_cfg);
                let icon_segment = if let Some(ref bg) = plug_bg {
                    let bg_code = self.apply_background_color(bg);
                    let icon_colored = if let Some(icon_color) = &mcp_cfg.colors.icon {
                        self.apply_color(&icon_glyph, Some(icon_color))
                            .replace("\x1b[0m", "")
                    } else {
                        icon_glyph
                    };
                    let block = format!("{} {} \x1b[49m", bg_code, icon_colored);
                    if self.config.style.separator == "\u{e0b0}" {
                        let arrow = self.create_powerline_arrow(Some(bg), None);
                        format!("{}{}", block, arrow)
                    } else {
                        block
                    }
                } else {
                    self.apply_color(&icon_glyph, mcp_cfg.colors.icon.as_ref())
                };

                // Multi-line wrapping: query terminal width and compute budgets
                let term_width = crossterm::terminal::size().ok().map(|(c, _)| c as usize);
                let first_line_budget = term_width
                    .map(|w| w.saturating_sub(display_width(&icon_segment) + 1))
                    .unwrap_or(usize::MAX);
                let body_lines = mcp::render_servers_lines(
                    &names,
                    self.config.style.mode,
                    term_width,
                    first_line_budget,
                );

                let mcp_line = if body_lines.len() <= 1 {
                    format!(
                        "{} {}",
                        icon_segment,
                        body_lines.first().map(|s| s.as_str()).unwrap_or("")
                    )
                } else {
                    let first = format!("{} {}", icon_segment, body_lines[0]);
                    let rest = body_lines[1..].join("\n");
                    format!("{}\n{}", first, rest)
                };

                return if line1.is_empty() {
                    mcp_line
                } else {
                    format!("{}\n{}", line1, mcp_line)
                };
            }
        }

        line1
    }

    /// Generate statusline for TUI preview with proper width calculation
    /// This method handles ANSI escape sequences properly for ratatui rendering
    pub fn generate_for_tui(
        &self,
        segments: Vec<(SegmentConfig, SegmentData)>,
    ) -> ratatui::text::Line<'static> {
        use ansi_to_tui::IntoText;
        use ratatui::text::{Line, Span};

        // Use the same generate method and convert to TUI.
        // Note: only the first line is returned; the MCP second line is intentionally dropped.
        let full_output = self.generate(segments);

        if let Ok(text) = full_output.into_text() {
            if let Some(line) = text.lines.into_iter().next() {
                return line;
            }
        }

        // Fallback to raw text
        Line::from(vec![Span::raw(full_output)])
    }

    /// Generate TUI-optimized text with intelligent wrapping by segment for preview
    pub fn generate_for_tui_preview(
        &self,
        segments: Vec<(SegmentConfig, SegmentData)>,
        max_width: u16,
    ) -> ratatui::text::Text<'_> {
        use ansi_to_tui::IntoText;
        use ratatui::text::{Line, Span, Text};

        // Mcp renders on a second physical line (via generate()); the TUI preview is
        // single-line only, so exclude it here to avoid invisible/broken rendering.
        let enabled_segments: Vec<_> = segments
            .into_iter()
            .filter(|(config, _)| config.enabled && config.id != crate::config::SegmentId::Mcp)
            .collect();

        if enabled_segments.is_empty() {
            return Text::from(vec![Line::default()]);
        }

        // Render each segment individually
        let mut rendered_segments = Vec::new();
        let mut segment_configs = Vec::new();

        for (config, data) in &enabled_segments {
            let rendered = self.render_segment(config, data);
            if !rendered.is_empty() {
                rendered_segments.push(rendered);
                segment_configs.push(config.clone());
            }
        }

        if rendered_segments.is_empty() {
            return Text::from(vec![Line::default()]);
        }

        // Pre-calculate separators between segments
        let mut separators = Vec::new();
        for i in 0..rendered_segments.len().saturating_sub(1) {
            let separator = if self.config.style.separator == "\u{e0b0}" {
                // Powerline arrows with color transition
                let prev_bg = segment_configs
                    .get(i)
                    .and_then(|config| config.colors.background.as_ref());
                let curr_bg = segment_configs
                    .get(i + 1)
                    .and_then(|config| config.colors.background.as_ref());
                self.create_powerline_arrow(prev_bg, curr_bg)
            } else {
                // Regular separators with white color
                format!("\x1b[37m{}\x1b[0m", self.config.style.separator)
            };
            separators.push(separator);
        }

        // Intelligent line wrapping by segment
        let mut lines: Vec<String> = Vec::new();
        let mut current_line = String::new();
        let mut current_width = 0usize;
        let max_w = max_width as usize;

        for i in 0..rendered_segments.len() {
            let segment = &rendered_segments[i];
            let segment_width = visible_width(segment);

            // Check if adding this segment would exceed max_width
            if current_width > 0 && current_width + segment_width > max_w {
                // Current line would overflow, start a new line
                lines.push(current_line.clone());
                current_line.clear();
                current_width = 0;
            }

            // Add the segment to current line
            current_line.push_str(segment);
            current_width += segment_width;

            // Handle separator if not the last segment
            if i < separators.len() {
                let separator = &separators[i];
                let separator_width = visible_width(separator);

                // Check if next segment exists
                if i + 1 < rendered_segments.len() {
                    let next_segment = &rendered_segments[i + 1];
                    let next_width = visible_width(next_segment);

                    // Check if separator AND next segment both fit
                    if current_width + separator_width + next_width <= max_w {
                        // Both fit, add separator and continue on same line
                        current_line.push_str(separator);
                        current_width += separator_width;
                    } else {
                        // Separator and/or next segment don't fit
                        // Don't add separator, just break line
                        lines.push(current_line.clone());
                        current_line.clear();
                        current_width = 0;
                    }
                }
            }
        }

        // Add the last line if it's not empty
        if !current_line.is_empty() {
            lines.push(current_line);
        }

        // Convert string lines to ratatui Text
        let mut tui_lines = Vec::new();
        for line in lines {
            if let Ok(text) = line.into_text() {
                for tui_line in text.lines {
                    tui_lines.push(tui_line);
                }
            } else {
                tui_lines.push(Line::from(vec![Span::raw(line)]));
            }
        }

        // Ensure we have at least one line
        if tui_lines.is_empty() {
            tui_lines.push(Line::default());
        }

        Text::from(tui_lines)
    }

    fn render_segment(&self, config: &SegmentConfig, data: &SegmentData) -> String {
        let icon = if let Some(dynamic_icon) = data.metadata.get("dynamic_icon") {
            dynamic_icon.clone()
        } else {
            self.get_icon(config)
        };

        // Two-tone model glyph: colored glyph + normal-colored version number.
        // Uses no-inner-reset to survive the bg-theme \x1b[0m stripping below.
        let two_tone_primary: Option<String> = if self.config.style.mode != StyleMode::Plain {
            if let (Some(glyph), Some(rest), Some(family)) = (
                data.metadata.get("model_glyph"),
                data.metadata.get("model_rest"),
                data.metadata.get("model_family"),
            ) {
                let glyph_color = match family.as_str() {
                    "opus" => (255u8, 191u8, 0u8),
                    "sonnet" => (0u8, 188u8, 212u8),
                    "haiku" => (76u8, 175u8, 80u8),
                    _ => (255u8, 255u8, 255u8),
                };
                let text_color = match &config.colors.text {
                    Some(AnsiColor::Rgb { r, g, b }) => {
                        format!("\x1b[38;2;{};{};{}m", r, g, b)
                    }
                    Some(AnsiColor::Color256 { c256 }) => {
                        format!("\x1b[38;5;{}m", c256)
                    }
                    Some(AnsiColor::Color16 { c16 }) => {
                        let code = if *c16 < 8 { 30 + c16 } else { 90 + (c16 - 8) };
                        format!("\x1b[{}m", code)
                    }
                    None => String::new(),
                };
                Some(format!(
                    "\x1b[38;2;{};{};{}m{} {}{}",
                    glyph_color.0, glyph_color.1, glyph_color.2, glyph, text_color, rest
                ))
            } else {
                None
            }
        } else {
            None
        };

        let primary_text = two_tone_primary.as_deref().unwrap_or(&data.primary);

        // Apply background color to the entire segment if set
        if let Some(bg_color) = &config.colors.background {
            let bg_code = self.apply_background_color(bg_color);

            // Build the entire segment content first
            let icon_colored = if let Some(icon_color) = &config.colors.icon {
                self.apply_color(&icon, Some(icon_color))
                    .replace("\x1b[0m", "")
            } else {
                icon.clone()
            };

            let text_styled = if two_tone_primary.is_some() {
                // Already has embedded color codes; strip only trailing reset so bg path works
                primary_text.replace("\x1b[0m", "")
            } else {
                self.apply_style(
                    primary_text,
                    config.colors.text.as_ref(),
                    config.styles.text_bold,
                )
                .replace("\x1b[0m", "")
            };

            let mut segment_content = format!(" {} {} ", icon_colored, text_styled);

            if !data.secondary.is_empty() {
                let secondary_styled = self
                    .apply_style(
                        &data.secondary,
                        config.colors.text.as_ref(),
                        config.styles.text_bold,
                    )
                    .replace("\x1b[0m", "");
                segment_content.push_str(&format!("{} ", secondary_styled));
            }

            // Apply background to the entire content and reset at the end
            format!("{}{}\x1b[49m", bg_code, segment_content)
        } else {
            // No background color, use original logic
            let icon_colored = self.apply_color(&icon, config.colors.icon.as_ref());
            let text_styled = if two_tone_primary.is_some() {
                format!("{}\x1b[0m", primary_text)
            } else {
                self.apply_style(
                    primary_text,
                    config.colors.text.as_ref(),
                    config.styles.text_bold,
                )
            };

            let mut segment = format!("{} {}", icon_colored, text_styled);

            if !data.secondary.is_empty() {
                segment.push_str(&format!(
                    " {}",
                    self.apply_style(
                        &data.secondary,
                        config.colors.text.as_ref(),
                        config.styles.text_bold
                    )
                ));
            }

            segment
        }
    }

    fn get_icon(&self, config: &SegmentConfig) -> String {
        match self.config.style.mode {
            StyleMode::Plain => config.icon.plain.clone(),
            StyleMode::NerdFont => config.icon.nerd_font.clone(),
            StyleMode::Powerline => config.icon.nerd_font.clone(), // Future: use Powerline icons
        }
    }

    fn apply_color(&self, text: &str, color: Option<&AnsiColor>) -> String {
        match color {
            Some(AnsiColor::Color16 { c16 }) => {
                let code = if *c16 < 8 { 30 + c16 } else { 90 + (c16 - 8) };
                format!("\x1b[{}m{}\x1b[0m", code, text)
            }
            Some(AnsiColor::Color256 { c256 }) => {
                format!("\x1b[38;5;{}m{}\x1b[0m", c256, text)
            }
            Some(AnsiColor::Rgb { r, g, b }) => {
                format!("\x1b[38;2;{};{};{}m{}\x1b[0m", r, g, b, text)
            }
            None => text.to_string(),
        }
    }

    fn apply_style(&self, text: &str, color: Option<&AnsiColor>, bold: bool) -> String {
        let mut codes = Vec::new();

        // Add style codes
        if bold {
            codes.push("1".to_string()); // Bold: \x1b[1m
        }

        // Add color codes
        match color {
            Some(AnsiColor::Color16 { c16 }) => {
                let color_code = if *c16 < 8 { 30 + c16 } else { 90 + (c16 - 8) };
                codes.push(color_code.to_string());
            }
            Some(AnsiColor::Color256 { c256 }) => {
                codes.push("38".to_string());
                codes.push("5".to_string());
                codes.push(c256.to_string());
            }
            Some(AnsiColor::Rgb { r, g, b }) => {
                codes.push("38".to_string());
                codes.push("2".to_string());
                codes.push(r.to_string());
                codes.push(g.to_string());
                codes.push(b.to_string());
            }
            None => {}
        }

        if codes.is_empty() {
            text.to_string()
        } else {
            format!("\x1b[{}m{}\x1b[0m", codes.join(";"), text)
        }
    }

    fn apply_background_color(&self, color: &AnsiColor) -> String {
        match color {
            AnsiColor::Color16 { c16 } => {
                let code = if *c16 < 8 { 40 + c16 } else { 100 + (c16 - 8) };
                format!("\x1b[{}m", code)
            }
            AnsiColor::Color256 { c256 } => {
                format!("\x1b[48;5;{}m", c256)
            }
            AnsiColor::Rgb { r, g, b } => {
                format!("\x1b[48;2;{};{};{}m", r, g, b)
            }
        }
    }

    /// Join segments with white separators (non-Powerline)
    fn join_with_white_separators(&self, rendered_segments: &[String]) -> String {
        if rendered_segments.is_empty() {
            return String::new();
        }

        // Use white color for separator
        let white_separator = format!("\x1b[37m{}\x1b[0m", self.config.style.separator);
        rendered_segments.join(&white_separator)
    }

    /// Join segments with Powerline arrow separators with proper color transitions
    fn join_with_powerline_arrows(
        &self,
        rendered_segments: &[String],
        segment_configs: &[(SegmentConfig, SegmentData)],
    ) -> String {
        if rendered_segments.is_empty() {
            return String::new();
        }

        if rendered_segments.len() == 1 {
            return rendered_segments[0].clone();
        }

        let mut result = rendered_segments[0].clone();

        for (i, _) in rendered_segments.iter().enumerate().skip(1) {
            let prev_bg = segment_configs
                .get(i - 1)
                .and_then(|(config, _)| config.colors.background.as_ref());
            let curr_bg = segment_configs
                .get(i)
                .and_then(|(config, _)| config.colors.background.as_ref());

            // Create Powerline arrow with color transition
            let arrow = self.create_powerline_arrow(prev_bg, curr_bg);

            result.push_str(&arrow);
            result.push_str(&rendered_segments[i]);
        }

        // Reset colors at the end
        result.push_str("\x1b[0m");
        result
    }

    /// Create a Powerline arrow with proper color transition
    fn create_powerline_arrow(
        &self,
        prev_bg: Option<&AnsiColor>,
        curr_bg: Option<&AnsiColor>,
    ) -> String {
        let arrow_char = "\u{e0b0}";

        match (prev_bg, curr_bg) {
            (Some(prev), Some(curr)) => {
                // Arrow foreground = previous segment's background
                // Arrow background = current segment's background
                let fg_code = self.color_to_foreground_code(prev);
                let bg_code = self.apply_background_color(curr);
                format!("{}{}{}\x1b[0m", bg_code, fg_code, arrow_char)
            }
            (Some(prev), None) => {
                // Previous segment has background, current doesn't
                let fg_code = self.color_to_foreground_code(prev);
                format!("{}{}\x1b[0m", fg_code, arrow_char)
            }
            (None, Some(curr)) => {
                // Current segment has background, previous doesn't
                let bg_code = self.apply_background_color(curr);
                format!("{}{}\x1b[0m", bg_code, arrow_char)
            }
            (None, None) => {
                // Neither segment has background color
                arrow_char.to_string()
            }
        }
    }

    /// Convert AnsiColor to foreground color code
    fn color_to_foreground_code(&self, color: &AnsiColor) -> String {
        match color {
            AnsiColor::Color16 { c16 } => {
                let code = if *c16 < 8 { 30 + c16 } else { 90 + (c16 - 8) };
                format!("\x1b[{}m", code)
            }
            AnsiColor::Color256 { c256 } => {
                format!("\x1b[38;5;{}m", c256)
            }
            AnsiColor::Rgb { r, g, b } => {
                format!("\x1b[38;2;{};{};{}m", r, g, b)
            }
        }
    }
}

pub fn collect_all_segments(
    config: &Config,
    input: &crate::config::InputData,
) -> Vec<(SegmentConfig, SegmentData)> {
    use crate::core::segments::*;

    let mut results = Vec::new();

    for segment_config in &config.segments {
        // Skip disabled segments to avoid unnecessary API requests
        if !segment_config.enabled {
            continue;
        }

        let segment_data = match segment_config.id {
            crate::config::SegmentId::Model => {
                let segment = ModelSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Directory => {
                let segment = DirectorySegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Git => {
                let show_sha = segment_config
                    .options
                    .get("show_sha")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let segment = GitSegment::new()
                    .with_sha(show_sha)
                    .with_mode(config.style.mode);
                segment.collect(input)
            }
            crate::config::SegmentId::ContextWindow => {
                let segment = ContextWindowSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Usage => {
                let segment = UsageSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Cost => {
                let segment = CostSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Session => {
                let segment = SessionSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::OutputStyle => {
                let segment = OutputStyleSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Update => {
                let segment = UpdateSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Mcp => {
                let segment = McpSegment::new();
                segment.collect(input)
            }
        };

        if let Some(data) = segment_data {
            results.push((segment_config.clone(), data));
        }
    }

    results
}
