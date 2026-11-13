//! Mermaid source drawn as SVG here in the app, by merman: no browser and
//! no node. The SVG keeps its labels as SVG text, which the app's SVG
//! renderer draws.

use merman::svg::SvgPipeline;
use merman::{
    MermaidEngine, OperationControl, RenderOutput, RenderRequest, Renderer,
    SourcePresentationConfig, SvgRequest,
};

/// The terminal's colours the diagram is drawn in, as 0xRRGGBB.
#[derive(Clone, Copy, Debug)]
pub struct Colours {
    pub bg: u32,
    pub fg: u32,
}

fn hex(c: u32) -> String {
    format!("#{c:06x}")
}

/// Mermaid's dark theme on a dark terminal, its default on a light one.
fn dark(c: u32) -> bool {
    let (r, g, b) = ((c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff);
    (r * 299 + g * 587 + b * 114) / 1000 < 128
}

/// The diagram as SVG, or mermaid's words for why not.
pub fn svg(source: &str, colours: Colours) -> Result<String, String> {
    let site = SourcePresentationConfig::from_value(serde_json::json!({
        "theme": if dark(colours.bg) { "dark" } else { "default" },
        "themeVariables": {
            "background": hex(colours.bg),
            "primaryTextColor": hex(colours.fg),
        }
    }));
    let renderer =
        Renderer::new().with_presentation_engine(MermaidEngine::new().with_site_config(site));
    let request = SvgRequest {
        pipeline: Some(SvgPipeline::resvg_safe()),
        ..Default::default()
    };
    match renderer.render(RenderRequest::svg(source, OperationControl::new(), request)) {
        Ok(RenderOutput::Svg(Some(svg))) => Ok(svg.svg().to_string()),
        Ok(_) => Err("not a diagram mermaid knows".into()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARK: Colours = Colours {
        bg: 0x101014,
        fg: 0xd0d0d0,
    };

    #[test]
    fn a_flowchart_draws_with_its_labels_as_svg_text() {
        let svg = svg("flowchart LR\n  A[Checkout] --> B[Build]", DARK).unwrap();
        assert!(svg.starts_with("<svg"), "{}", &svg[..60]);
        assert!(svg.contains("Checkout") && svg.contains("Build"));
        assert!(
            !svg.contains("foreignObject"),
            "labels the renderer cannot draw"
        );
    }

    #[test]
    fn source_mermaid_cannot_read_says_why() {
        assert!(svg("flowchart LR\n  A[ --> ", DARK).is_err());
    }
}
