//! QR codes for the panels (login links, pairing codes).

use base64::Engine;

/// An SVG QR code as a `data:` URL. Returned inline, never written to a
/// file: what it encodes is often a secret.
pub fn svg_data_url(data: &str) -> anyhow::Result<String> {
    let code = qrcode::QrCode::new(data.as_bytes())?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(256, 256)
        .quiet_zone(true)
        .build();
    let encoded = base64::engine::general_purpose::STANDARD.encode(svg.as_bytes());
    Ok(format!("data:image/svg+xml;base64,{encoded}"))
}
