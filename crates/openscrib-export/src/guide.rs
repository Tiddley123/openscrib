//! The written guide: Markdown (folder with images), HTML (one
//! self-contained file) and PDF (via Typst, see `pdf.rs`).

use std::fs;
use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use base64::Engine;
use image::ImageFormat;
use openscrib_core::{Manifest, ProjectDir};
use pulldown_cmark::{Event, Parser, Tag, TagEnd, html};

use crate::render::{RenderOptions, render_step};

pub const DEFAULT_TITLE: &str = "Recorded guide";

pub struct GuideStep {
    pub number: usize,
    /// Step text as written in the editor (inline Markdown).
    pub text: String,
    pub image: Option<GuideImage>,
}

pub struct GuideImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub struct Guide {
    pub title: String,
    /// `#rrggbb` used for step badges.
    pub accent: String,
    pub steps: Vec<GuideStep>,
}

impl Guide {
    pub fn build(dir: &ProjectDir, manifest: &Manifest, opts: &RenderOptions) -> Result<Self> {
        let mut steps = Vec::with_capacity(manifest.steps.len());
        for (i, step) in manifest.steps.iter().enumerate() {
            let image = match render_step(dir, manifest, step, opts)? {
                Some(img) => {
                    let mut png = Vec::new();
                    img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
                    Some(GuideImage { png, width: img.width(), height: img.height() })
                }
                None => None,
            };
            steps.push(GuideStep { number: i + 1, text: step.text.trim().to_string(), image });
        }
        let title = manifest
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(DEFAULT_TITLE)
            .to_string();
        Ok(Self { title, accent: manifest.accent_hex(), steps })
    }

    pub fn image_name(number: usize) -> String {
        format!("step-{number:02}.png")
    }

    /// `out_dir/guide.md` plus `out_dir/images/step-NN.png`.
    pub fn write_markdown(&self, out_dir: &Path) -> Result<()> {
        fs::create_dir_all(out_dir.join("images"))?;
        let mut md = format!("# {}\n\n", self.title.replace('\n', " "));
        for s in &self.steps {
            md.push_str(&format!("{}. {}\n", s.number, one_line(&s.text)));
            if let Some(img) = &s.image {
                let name = Self::image_name(s.number);
                fs::write(out_dir.join("images").join(&name), &img.png)?;
                md.push_str(&format!("\n   ![Step {}](images/{name})\n", s.number));
            }
            md.push('\n');
        }
        fs::write(out_dir.join("guide.md"), md).context("writing guide.md")?;
        Ok(())
    }

    /// One HTML file with the images embedded, so it can be emailed or
    /// dropped on an intranet as-is.
    pub fn write_html(&self, out_file: &Path) -> Result<()> {
        let b64 = base64::engine::general_purpose::STANDARD;
        let mut body = String::new();
        for s in &self.steps {
            body.push_str(&format!(
                "<li class=\"step\"><div class=\"head\"><span class=\"n\">{}</span><div class=\"text\">{}</div></div>",
                s.number,
                inline_html(&s.text)
            ));
            if let Some(img) = &s.image {
                body.push_str(&format!(
                    "<img src=\"data:image/png;base64,{}\" width=\"{}\" height=\"{}\" alt=\"Step {}\">",
                    b64.encode(&img.png),
                    img.width,
                    img.height,
                    s.number
                ));
            }
            body.push_str("</li>\n");
        }
        let title = escape(&self.title);
        let html = HTML_TEMPLATE
            .replace("{{accent}}", &escape(&self.accent))
            .replace("{{title}}", &title)
            .replace("{{count}}", &format!("{} step{}", self.steps.len(), if self.steps.len() == 1 { "" } else { "s" }))
            .replace("{{steps}}", &body);
        fs::write(out_file, html).with_context(|| format!("writing {}", out_file.display()))?;
        Ok(())
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Render inline Markdown to HTML without the wrapping paragraph. Raw HTML in
/// step text is escaped, not passed through.
pub fn inline_html(md: &str) -> String {
    let events = Parser::new(md).map(|e| match e {
        Event::Html(h) | Event::InlineHtml(h) => Event::Text(h),
        other => other,
    });
    let mut out = String::new();
    html::push_html(&mut out, events);
    let trimmed = out.trim();
    trimmed
        .strip_prefix("<p>")
        .and_then(|s| s.strip_suffix("</p>"))
        .unwrap_or(trimmed)
        .to_string()
}

/// Inline Markdown to Typst markup. Text goes in as string literals, so no
/// character in a step can be misread as Typst syntax.
pub fn inline_typst(md: &str) -> String {
    let mut out = String::new();
    for e in Parser::new(md) {
        match e {
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => out.push_str(&format!("#{}", typst_str(&t))),
            Event::Code(t) => out.push_str(&format!("#raw({})", typst_str(&t))),
            Event::SoftBreak | Event::HardBreak => out.push(' '),
            Event::Start(Tag::Strong) => out.push_str("#strong["),
            Event::Start(Tag::Emphasis) => out.push_str("#emph["),
            Event::End(TagEnd::Strong | TagEnd::Emphasis) => out.push(']'),
            Event::End(TagEnd::Paragraph) => out.push(' '),
            _ => {}
        }
    }
    out.trim().to_string()
}

pub fn typst_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const HTML_TEMPLATE: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="generator" content="OpenSCRIB">
<title>{{title}}</title>
<style>
:root { --bg: #fafaf9; --card: #ffffff; --ink: #1c1c1e; --muted: #6e6e73; --line: #e5e5e3; --accent: {{accent}}; }
@media (prefers-color-scheme: dark) {
  :root { --bg: #141416; --card: #1c1c1f; --ink: #f2f2f2; --muted: #9a9aa0; --line: #2c2c30; }
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--ink);
  font: 16px/1.55 "Inter", "Segoe UI", system-ui, -apple-system, sans-serif; }
main { max-width: 980px; margin: 0 auto; padding: 56px 24px 80px; }
h1 { font-size: 34px; letter-spacing: -0.02em; line-height: 1.15; margin: 0 0 6px; }
.meta { color: var(--muted); font-size: 14px; margin: 0 0 40px; }
ol { list-style: none; padding: 0; margin: 0; display: grid; gap: 28px; }
.step { background: var(--card); border: 1px solid var(--line); border-radius: 14px; padding: 20px; }
.head { display: flex; gap: 14px; align-items: flex-start; }
.n { flex: none; width: 30px; height: 30px; border-radius: 50%; background: var(--accent); color: #fff;
  font-weight: 700; font-size: 14px; display: grid; place-items: center; }
.text { padding-top: 3px; font-size: 17px; }
code { font: 0.92em ui-monospace, "Cascadia Mono", Consolas, monospace; background: var(--bg);
  border: 1px solid var(--line); border-radius: 5px; padding: 1px 5px; }
img { display: block; width: 100%; height: auto; margin-top: 16px; border-radius: 10px; border: 1px solid var(--line); }
footer { margin-top: 48px; color: var(--muted); font-size: 13px; }
@media print { body { background: #fff; } .step { break-inside: avoid; border: none; padding: 0; } }
</style>
</head>
<body>
<main>
<h1>{{title}}</h1>
<p class="meta">{{count}}</p>
<ol>
{{steps}}</ol>
<footer>Made with OpenSCRIB</footer>
</main>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_markdown_to_html() {
        assert_eq!(inline_html("Click **Save** then `Ctrl+S`."), "Click <strong>Save</strong> then <code>Ctrl+S</code>.");
        assert_eq!(inline_html("<script>x</script>"), "&lt;script&gt;x&lt;/script&gt;");
    }

    #[test]
    fn inline_markdown_to_typst() {
        assert_eq!(
            inline_typst("Click **Save \"now\"** #1 `a*b`"),
            r##"#"Click "#strong[#"Save \"now\""]#" #1 "#raw("a*b")"##
        );
    }
}
