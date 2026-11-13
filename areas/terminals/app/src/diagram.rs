//! Mermaid source drawn here in the app, by merman, and painted to pixels
//! by resvg: no browser, no node, and no system fonts. Labels are measured
//! and drawn in the app's own JetBrains Mono, from the files the app
//! carries, so a diagram looks the same on every system.

use merman::svg::SvgPipeline;
use merman::{
    MermaidEngine, OperationControl, RenderOutput, RenderRequest, Renderer,
    SourcePresentationConfig, SvgEnvironment, SvgRequest,
};
use merman_render::environment::{
    MeasurementProfileId, TextMeasurementPolicy, TextMeasurementProfile,
    TextMeasurementProfileIdentity,
};
use merman_render::text::{TextMeasurer, TextMetrics, TextStyle, WrapMode};
use parvi_app_kit::theme::{FAMILY, FAMILY_FILES};
use std::sync::{Arc, OnceLock};

/// The terminal's colours the diagram is drawn in, as 0xRRGGBB.
#[derive(Clone, Copy, Debug)]
pub struct Colours {
    pub bg: u32,
    pub fg: u32,
}

/// JetBrains Mono's measures, as shares of the font size: every letter's
/// advance, and a line.
const ADVANCE: f64 = 0.6;
const LINE: f64 = 1.32;
/// The most pixels a drawn diagram has along a side.
const MOST_PX: f32 = 8192.;

fn hex(c: u32) -> String {
    format!("#{c:06x}")
}

/// Mermaid's dark theme on a dark terminal, its default on a light one.
fn dark(c: u32) -> bool {
    let (r, g, b) = ((c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff);
    (r * 299 + g * 587 + b * 114) / 1000 < 128
}

/// Labels measured as JetBrains Mono sets them: one advance a letter, two
/// for the wide ones another font draws.
struct Mono;

impl Mono {
    fn width(text: &str, size: f64) -> f64 {
        let columns: usize = text
            .chars()
            .map(|c| {
                if (c as u32) >= 0x1100 && !c.is_ascii() {
                    2
                } else {
                    1
                }
            })
            .sum();
        columns as f64 * ADVANCE * size
    }

    /// Lines broken at spaces so none is wider than `most`, as many as the
    /// text's own breaks make at least.
    fn lines(text: &str, size: f64, most: Option<f64>) -> Vec<String> {
        let mut out = Vec::new();
        for given in text.split('\n') {
            let Some(most) = most else {
                out.push(given.to_string());
                continue;
            };
            let mut line = String::new();
            for word in given.split(' ') {
                let tried = if line.is_empty() {
                    word.to_string()
                } else {
                    format!("{line} {word}")
                };
                if !line.is_empty() && Self::width(&tried, size) > most {
                    out.push(std::mem::replace(&mut line, word.to_string()));
                } else {
                    line = tried;
                }
            }
            out.push(line);
        }
        out
    }
}

impl TextMeasurer for Mono {
    fn measure(&self, text: &str, style: &TextStyle) -> TextMetrics {
        self.measure_wrapped(text, style, None, WrapMode::SvgLike)
    }

    fn measure_wrapped(
        &self,
        text: &str,
        style: &TextStyle,
        max_width: Option<f64>,
        _: WrapMode,
    ) -> TextMetrics {
        let size = style.font_size;
        let lines = Self::lines(text, size, max_width);
        TextMetrics {
            width: lines
                .iter()
                .map(|l| Self::width(l, size))
                .fold(0., f64::max),
            height: lines.len() as f64 * LINE * size,
            line_count: lines.len(),
        }
    }
}

fn measured() -> Result<SvgEnvironment, String> {
    let id = MeasurementProfileId::new("parvi.jetbrains-mono").map_err(|e| e.to_string())?;
    let identity = TextMeasurementProfileIdentity::new(id, "1").map_err(|e| e.to_string())?;
    let profile = TextMeasurementProfile::new(identity, Arc::new(Mono));
    Ok(SvgEnvironment::deterministic()
        .with_text_measurement_policy(TextMeasurementPolicy::uniform(profile)))
}

/// The diagram as SVG, or mermaid's words for why not.
pub fn svg(source: &str, colours: Colours) -> Result<String, String> {
    let site = SourcePresentationConfig::from_value(serde_json::json!({
        "theme": if dark(colours.bg) { "dark" } else { "default" },
        "fontFamily": FAMILY,
        "themeVariables": {
            "fontFamily": FAMILY,
            "background": hex(colours.bg),
            "primaryTextColor": hex(colours.fg),
            "edgeLabelBackground": hex(colours.bg),
        }
    }));
    let renderer =
        Renderer::new().with_presentation_engine(MermaidEngine::new().with_site_config(site));
    let request = SvgRequest {
        environment: measured()?,
        pipeline: Some(SvgPipeline::resvg_safe()),
        ..Default::default()
    };
    match renderer.render(RenderRequest::svg(source, OperationControl::new(), request)) {
        Ok(RenderOutput::Svg(Some(svg))) => Ok(svg.svg().to_string()),
        Ok(_) => Err("not a diagram mermaid knows".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// A diagram in pixels: its size as drawn at 1:1, and its pixels at
/// `scale` times that, BGRA as the window takes them.
pub struct Drawn {
    pub width: f32,
    pub height: f32,
    pub pixels: image::RgbaImage,
}

fn options() -> &'static resvg::usvg::Options<'static> {
    static OPTIONS: OnceLock<resvg::usvg::Options<'static>> = OnceLock::new();
    OPTIONS.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        for file in FAMILY_FILES {
            db.load_font_data(file.to_vec());
        }
        db.set_monospace_family(FAMILY);
        db.set_sans_serif_family(FAMILY);
        db.set_serif_family(FAMILY);
        resvg::usvg::Options {
            font_family: FAMILY.into(),
            fontdb: Arc::new(db),
            // Whatever family the SVG names, its letters are the app's.
            style_sheet: Some(format!("text, tspan {{ font-family: '{FAMILY}'; }}")),
            ..Default::default()
        }
    })
}

/// Paints `svg` at `scale` times its size, no side past the most pixels.
pub fn paint(svg: &str, scale: f32) -> Result<Drawn, String> {
    let tree = resvg::usvg::Tree::from_str(svg, options()).map_err(|e| e.to_string())?;
    let size = tree.size();
    let scale = scale
        .min(MOST_PX / size.width())
        .min(MOST_PX / size.height());
    let (w, h) = (
        (size.width() * scale).ceil().max(1.) as u32,
        (size.height() * scale).ceil().max(1.) as u32,
    );
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or("a diagram of no size")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let mut pixels =
        image::RgbaImage::from_raw(w, h, pixmap.take()).ok_or("pixels that do not fit")?;
    // Premultiplied RGBA to the window's straight BGRA.
    for p in pixels.pixels_mut() {
        let [r, g, b, a] = p.0;
        let un = |c: u8| {
            if a == 0 {
                0
            } else {
                ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8
            }
        };
        p.0 = [un(b), un(g), un(r), a];
    }
    Ok(Drawn {
        width: size.width(),
        height: size.height(),
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARK: Colours = Colours {
        bg: 0x101014,
        fg: 0xd0d0d0,
    };

    #[test]
    fn a_flowchart_draws_with_its_labels_in_the_apps_font() {
        let svg = svg("flowchart LR\n  A[Checkout] --> B[Build]", DARK).unwrap();
        assert!(svg.starts_with("<svg"), "{}", &svg[..60]);
        assert!(svg.contains("Checkout") && svg.contains("Build"));
        assert!(svg.contains(FAMILY), "labels name the app's font");
        assert!(
            !svg.contains("foreignObject"),
            "labels the renderer cannot draw"
        );
    }

    #[test]
    fn source_mermaid_cannot_read_says_why() {
        assert!(svg("flowchart LR\n  A[ --> ", DARK).is_err());
    }

    #[test]
    fn labels_are_measured_a_letter_at_a_time_and_broken_at_spaces() {
        assert_eq!(Mono::width("Build", 10.), 30.);
        assert_eq!(
            Mono::lines("one two three", 10., Some(45.)),
            ["one two", "three"]
        );
        assert_eq!(Mono::lines("a\nb", 10., None), ["a", "b"]);
    }

    #[test]
    fn letters_are_drawn_from_the_apps_own_font_whatever_the_svg_names() {
        let text = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="30"><text x="2" y="20" font-family="NoSuchFont" font-size="16" fill="#fff">Hi</text></svg>"##;
        let drawn = paint(text, 1.).unwrap();
        assert!(
            drawn.pixels.pixels().any(|p| p.0[3] > 200),
            "no letters inked"
        );
    }

    #[test]
    fn a_diagram_paints_at_the_scale_asked_with_its_letters_inked() {
        let svg = svg("flowchart LR\n  A[Checkout] --> B[Build]", DARK).unwrap();
        let one = paint(&svg, 1.).unwrap();
        let two = paint(&svg, 2.).unwrap();
        assert_eq!(two.pixels.width(), (one.width * 2.).ceil() as u32);
        assert!(
            one.pixels.pixels().filter(|p| p.0[3] > 0).count() > 100,
            "something is drawn"
        );
    }
}
