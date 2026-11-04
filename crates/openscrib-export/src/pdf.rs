//! PDF export: the guide is laid out as a Typst document and compiled
//! in-process. System fonts are preferred (Segoe UI on Windows) with Typst's
//! embedded fonts as the fallback, so it works offline everywhere.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::{self, FilterType};
use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst_layout::PagedDocument;
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::fonts::{self, FontStore};

use crate::guide::{Guide, GuideImage, inline_typst, typst_str};
use crate::icons::callout_style;
use openscrib_core::Callout;

/// Content width of an A4 page with our margins, in mm.
const CONTENT_W_MM: f32 = 174.0;
/// Tallest an image may be so a step (badge + text + image) fits one page.
const MAX_IMG_H_MM: f32 = 175.0;
/// Image quality that keeps screenshots lossless.
pub const LOSSLESS: u8 = 100;
pub const DEFAULT_QUALITY: u8 = 85;
/// Image width in pixels at quality `q` is `MIN_IMG_W + PX_PER_QUALITY * q`:
/// about 130 dpi on the page at the bottom of the scale, 200 dpi near the top.
const MIN_IMG_W: u32 = 900;
const PX_PER_QUALITY: u32 = 5;

static FONTS: OnceLock<FontStore> = OnceLock::new();

fn font_store() -> &'static FontStore {
    FONTS.get_or_init(|| {
        let mut store = FontStore::new();
        store.extend(fonts::system());
        store.extend(fonts::embedded());
        store
    })
}

/// An image as the PDF stores it. At 100 it stays a lossless PNG; below that
/// it is scaled for the page and saved as a JPEG of that quality.
fn pdf_image(img: &GuideImage, quality: u8) -> Result<Vec<u8>> {
    if quality >= LOSSLESS {
        return Ok(img.png.clone());
    }
    let rgba = image::load_from_memory(&img.png)?.to_rgba8();
    let width = (MIN_IMG_W + PX_PER_QUALITY * quality as u32).min(rgba.width());
    let height = (rgba.height() as u64 * width as u64 / rgba.width() as u64).max(1) as u32;
    let scaled = if width < rgba.width() { imageops::resize(&rgba, width, height, FilterType::Triangle) } else { rgba };
    let rgb = image::DynamicImage::ImageRgba8(scaled).to_rgb8();
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, quality.max(1)).encode_image(&rgb)?;
    Ok(jpeg)
}

fn pdf_image_name(number: usize, quality: u8) -> String {
    let name = Guide::image_name(number);
    if quality >= LOSSLESS { name } else { name.replace(".png", ".jpg") }
}

impl Guide {
    /// `quality` is 1..=100 for the screenshots, see [`pdf_image`].
    pub fn write_pdf(&self, out_file: &Path, quality: u8) -> Result<()> {
        fs::write(out_file, self.to_pdf(quality)?)?;
        Ok(())
    }

    /// Render each PDF page to a PNG, e.g. for previews. `pixel_per_pt` 2.0
    /// is roughly 144 dpi.
    pub fn pdf_page_pngs(&self, pixel_per_pt: f64, quality: u8) -> Result<Vec<Vec<u8>>> {
        let doc = self.layout(quality)?;
        let opts = typst_render::RenderOptions { pixel_per_pt: pixel_per_pt.into(), ..Default::default() };
        doc.pages()
            .iter()
            .map(|p| typst_render::render(p, &opts).encode_png().map_err(|e| anyhow!("encoding page: {e}")))
            .collect()
    }

    fn layout(&self, quality: u8) -> Result<PagedDocument> {
        let mut files = HashMap::new();
        for s in &self.steps {
            if let Some(img) = &s.image {
                files.insert(pdf_image_name(s.number, quality), Bytes::new(pdf_image(img, quality)?));
            }
        }
        let world = GuideWorld::new(self.typst_source(quality), files);
        typst::compile(&world).output.map_err(|errs| {
            anyhow!(
                "PDF layout failed: {}",
                errs.iter().map(|e| e.message.to_string()).collect::<Vec<_>>().join("; ")
            )
        })
    }

    pub fn to_pdf(&self, quality: u8) -> Result<Vec<u8>> {
        let doc = self.layout(quality)?;
        typst_pdf::pdf(&doc, &typst_pdf::PdfOptions::default()).map_err(|errs| {
            anyhow!(
                "PDF export failed: {}",
                errs.iter().map(|e| e.message.to_string()).collect::<Vec<_>>().join("; ")
            )
        })
    }

    pub fn typst_source(&self, quality: u8) -> String {
        let mut src = format!(
            r##"#set document(title: {title})
#set page(paper: "a4", margin: (x: 18mm, top: 16mm, bottom: 18mm),
  footer: context [#set text(8pt, fill: luma(150)); OpenSCRIB #h(1fr) #counter(page).display()])
#set text(font: ("Segoe UI", "Inter", "Helvetica Neue", "Arial", "Libertinus Serif"), size: 10.5pt, fill: rgb("#1c1c1e"))
#set par(leading: 0.6em)
#show raw: it => box(fill: luma(244), inset: (x: 3pt), outset: (y: 2pt), radius: 2pt, text(font: ("Cascadia Mono", "Consolas", "DejaVu Sans Mono"), size: 0.92em, it))

#text(22pt, weight: "bold", tracking: -0.3pt)[#{title}]
#v(-4pt)
#text(9pt, fill: luma(120))[#{count}]
#v(16pt)
"##,
            title = typst_str(&self.title),
            count = typst_str(&format!("{} step{}", self.steps.len(), if self.steps.len() == 1 { "" } else { "s" })),
        );

        for s in &self.steps {
            let image = s.image.as_ref().map(|img| {
                let aspect = img.height as f32 / img.width as f32;
                let size = if CONTENT_W_MM * aspect > MAX_IMG_H_MM {
                    format!("height: {MAX_IMG_H_MM}mm")
                } else {
                    "width: 100%".into()
                };
                format!(
                    "#v(8pt)\n  #box(stroke: 0.5pt + luma(215), radius: 4pt, clip: true, image({}, {size}))\n",
                    typst_str(&pdf_image_name(s.number, quality))
                )
            });
            let badge = format!(
                r##"box(width: 20pt, height: 20pt, radius: 10pt, fill: rgb({accent}),
      align(center + horizon, text(fill: white, weight: "bold", size: 9.5pt)[{n}]))"##,
                n = s.number,
                accent = typst_str(&self.accent),
            );
            let head = match s.callout {
                None => format!("#grid(columns: (20pt, 1fr), column-gutter: 9pt,\n    {badge},\n    pad(top: 2.5pt)[{}])", inline_typst(&s.text)),
                Some(kind) => {
                    let (color, label, svg) = callout_style(kind, &self.accent);
                    let svg = svg.replace("currentColor", &color);
                    let label = if label.is_empty() {
                        String::new()
                    } else {
                        format!("#text(fill: rgb({}), weight: \"bold\", size: 8pt, tracking: 0.6pt, upper({}))\\ ", typst_str(&color), typst_str(label))
                    };
                    let text = if kind == Callout::Quote {
                        format!("#text(size: 12pt, style: \"italic\")[{}]", inline_typst(&s.text))
                    } else {
                        inline_typst(&s.text)
                    };
                    format!(
                        r##"#block(width: 100%, fill: rgb({c}).lighten(90%), stroke: (left: 3pt + rgb({c})), inset: 10pt, radius: (right: 4pt))[
    #grid(columns: (20pt, 1fr, 40pt), column-gutter: 9pt, align: (left, horizon, right),
      {badge},
      [{label}{text}],
      image(bytes({svg}), format: "svg", width: 34pt))]"##,
                        c = typst_str(&color),
                        svg = typst_str(&svg),
                    )
                }
            };
            src.push_str(&format!(
                "#block(breakable: false, below: 20pt)[\n  {head}\n  {image}]\n",
                image = image.unwrap_or_default(),
            ));
        }
        src
    }
}

struct GuideWorld {
    library: LazyHash<Library>,
    main: Source,
    files: HashMap<String, Bytes>,
}

impl GuideWorld {
    fn new(source: String, files: HashMap<String, Bytes>) -> Self {
        let path = VirtualPath::new("main.typ").expect("valid path");
        let main = Source::new(FileId::new(RootedPath::new(VirtualRoot::Project, path)), source);
        Self { library: LazyHash::new(Library::default()), main, files }
    }
}

fn id_name(id: FileId) -> String {
    id.get().vpath().get_without_slash().to_string()
}

impl World for GuideWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        font_store().book()
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else {
            Err(FileError::NotFound(id_name(id).into()))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        let name = id_name(id);
        self.files
            .get(&name)
            .cloned()
            .ok_or_else(|| FileError::NotFound(name.into()))
    }

    fn font(&self, index: usize) -> Option<Font> {
        font_store().font(index)
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide::{GuideImage, GuideStep};

    #[test]
    fn compiles_a_guide_to_pdf() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(400, 300, image::Rgba([200, 220, 240, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let guide = Guide {
            title: "Reset a \"password\" #1".into(),
            accent: "#0a84ff".into(),
            steps: vec![
                GuideStep {
                    number: 1,
                    text: "Click **Settings** (gear icon) and press `Ctrl+S`.".into(),
                    image: Some(GuideImage { png: png.clone(), width: 400, height: 300 }),
                    callout: Some(Callout::Warning),
                },
                GuideStep { number: 2, text: "Type `$HOME/*` [done]".into(), image: None, callout: Some(Callout::Quote) },
                // A tall portrait-monitor shot must not overflow the page.
                GuideStep { number: 3, text: "Tall".into(), image: Some(GuideImage { png, width: 400, height: 1400 }), callout: None },
            ],
        };
        let pdf = guide.to_pdf(LOSSLESS).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(pdf.len() > 1000);
    }

    #[test]
    fn lower_quality_means_a_smaller_pdf() {
        // Busy like a screenshot, so PNG can't flatten it to nothing.
        let shot = image::RgbaImage::from_fn(2000, 1200, |x, y| {
            let n = x.wrapping_mul(31) ^ y.wrapping_mul(17) ^ (x * y);
            image::Rgba([n as u8, (n >> 3) as u8, (x / 8) as u8, 255])
        });
        let mut png = Vec::new();
        shot.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let image = GuideImage { png, width: 2000, height: 1200 };
        let guide = Guide {
            title: "Sizes".into(),
            accent: "#0a84ff".into(),
            steps: vec![GuideStep { number: 1, text: "Click.".into(), image: Some(image), callout: None }],
        };
        let sizes: Vec<usize> = [20, 85, LOSSLESS].iter().map(|&q| guide.to_pdf(q).unwrap().len()).collect();
        assert!(sizes[0] < sizes[1] && sizes[1] < sizes[2], "{sizes:?}");
    }
}
