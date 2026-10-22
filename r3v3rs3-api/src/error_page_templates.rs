// Copyright 2026 r3v3rs3 Authors
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};

/// Metadata describing a built-in error page template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateMetadata {
    /// Unique identifier for the template (e.g. "minimal_dark").
    pub id: &'static str,
    /// Display name of the template.
    pub name: &'static str,
    /// Short description of the template design and purpose.
    pub description: &'static str,
    /// Visual style category (e.g. "dark", "glass", "terminal", "illustration", "maintenance").
    pub category: &'static str,
}

/// List of all built-in custom error page templates.
pub const TEMPLATES: &[TemplateMetadata] = &[
    TemplateMetadata {
        id: "minimal_dark",
        name: "Minimalist Dark",
        description: "Sleek dark theme with subtle radiant glow, status badge, and clean action button.",
        category: "dark",
    },
    TemplateMetadata {
        id: "modern_glass",
        name: "Modern Glassmorphism",
        description: "Frosted glass card with vibrant gradient typography and elegant styling.",
        category: "glass",
    },
    TemplateMetadata {
        id: "cyber_terminal",
        name: "Developer Terminal",
        description: "DevOps / CLI terminal console with mock HTTP headers, diagnostics, and blinking cursor.",
        category: "terminal",
    },
    TemplateMetadata {
        id: "playful_illustration",
        name: "Illustration Card",
        description: "Centered modern card featuring clean vector SVG graphics and friendly messaging.",
        category: "illustration",
    },
    TemplateMetadata {
        id: "maintenance_status",
        name: "Maintenance Notice",
        description: "Amber maintenance notice with gear illustration, live status badge, and retry button.",
        category: "maintenance",
    },
];

/// Returns the standard HTTP error title and human-friendly description for a status code.
pub fn status_title_and_description(status: u16) -> (&'static str, &'static str) {
    match status {
        400 => (
            "Bad Request",
            "The server could not understand the request due to invalid syntax or parameters.",
        ),
        401 => (
            "Unauthorized",
            "Authentication credentials are missing or invalid for this protected resource.",
        ),
        403 => (
            "Access Forbidden",
            "You do not have administrative permission to view or access this endpoint.",
        ),
        404 => (
            "Page Not Found",
            "The requested page or resource could not be found or has been relocated.",
        ),
        405 => (
            "Method Not Allowed",
            "The HTTP request method is not supported for this requested URL.",
        ),
        408 => (
            "Request Timeout",
            "The server timed out waiting for the complete client request.",
        ),
        429 => (
            "Too Many Requests",
            "Rate limit exceeded. Please wait a moment before sending more requests.",
        ),
        500 => (
            "Internal Server Error",
            "The server encountered an internal issue and could not fulfill the request.",
        ),
        502 => (
            "Bad Gateway",
            "The reverse proxy received an invalid response or cannot connect to the upstream server.",
        ),
        503 => (
            "Service Unavailable",
            "The service is temporarily overloaded or undergoing scheduled maintenance.",
        ),
        504 => (
            "Gateway Timeout",
            "The upstream service did not respond within the configured gateway timeout limit.",
        ),
        _ => (
            "Error Encountered",
            "An unexpected HTTP error occurred while processing this request.",
        ),
    }
}

/// Renders a full, responsive, self-contained HTML/CSS error page for the specified template ID and HTTP status code.
pub fn render_template(template_id: &str, status: u16) -> String {
    let (title, description) = status_title_and_description(status);
    match template_id {
        "modern_glass" => render_modern_glass(status, title, description),
        "cyber_terminal" => render_cyber_terminal(status, title, description),
        "playful_illustration" => render_playful_illustration(status, title, description),
        "maintenance_status" => render_maintenance_status(status, title, description),
        _ => render_minimal_dark(status, title, description),
    }
}

fn render_minimal_dark(status: u16, title: &str, description: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{status} - {title}</title>
  <style>
    * {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      background-color: #090a0f;
      color: #f3f4f6;
      min-height: 100vh;
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 24px;
      position: relative;
      overflow: hidden;
    }}
    .glow {{
      position: absolute;
      width: 480px;
      height: 480px;
      background: radial-gradient(circle, rgba(99, 102, 241, 0.18) 0%, rgba(99, 102, 241, 0) 70%);
      top: 50%;
      left: 50%;
      transform: translate(-50%, -50%);
      pointer-events: none;
      z-index: 0;
    }}
    .container {{
      max-width: 520px;
      width: 100%;
      text-align: center;
      position: relative;
      z-index: 1;
    }}
    .badge {{
      display: inline-block;
      padding: 6px 14px;
      font-size: 13px;
      font-weight: 700;
      letter-spacing: 0.08em;
      text-transform: uppercase;
      color: #818cf8;
      background: rgba(99, 102, 241, 0.12);
      border: 1px solid rgba(99, 102, 241, 0.3);
      border-radius: 9999px;
      margin-bottom: 20px;
    }}
    .status-code {{
      font-size: clamp(64px, 15vw, 110px);
      font-weight: 900;
      line-height: 1;
      letter-spacing: -0.04em;
      color: #ffffff;
      margin-bottom: 12px;
    }}
    h1 {{
      font-size: 24px;
      font-weight: 700;
      color: #f9fafb;
      margin-bottom: 12px;
    }}
    p {{
      font-size: 15px;
      line-height: 1.6;
      color: #9ca3af;
      margin-bottom: 32px;
    }}
    .actions {{
      display: flex;
      gap: 12px;
      justify-content: center;
      flex-wrap: wrap;
    }}
    .btn {{
      padding: 12px 24px;
      font-size: 14px;
      font-weight: 600;
      border-radius: 10px;
      text-decoration: none;
      cursor: pointer;
      transition: all 0.2s ease;
      display: inline-flex;
      align-items: center;
      justify-content: center;
    }}
    .btn-primary {{
      background: #6366f1;
      color: #ffffff;
      border: none;
    }}
    .btn-primary:hover {{
      background: #4f46e5;
      transform: translateY(-1px);
    }}
    .btn-secondary {{
      background: rgba(255, 255, 255, 0.06);
      color: #e5e7eb;
      border: 1px solid rgba(255, 255, 255, 0.12);
    }}
    .btn-secondary:hover {{
      background: rgba(255, 255, 255, 0.1);
      transform: translateY(-1px);
    }}
    .footer {{
      margin-top: 48px;
      font-size: 12px;
      color: #6b7280;
    }}
  </style>
</head>
<body>
  <div class="glow"></div>
  <div class="container">
    <div class="badge">HTTP Error {status}</div>
    <div class="status-code">{status}</div>
    <h1>{title}</h1>
    <p>{description}</p>
    <div class="actions">
      <a href="/" class="btn btn-primary">Return Home</a>
      <button onclick="window.location.reload()" class="btn btn-secondary">Retry Request</button>
    </div>
    <div class="footer">Protected by r3v3rs3</div>
  </div>
</body>
</html>"#
    )
}

fn render_modern_glass(status: u16, title: &str, description: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{status} - {title}</title>
  <style>
    * {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
      background: linear-gradient(135deg, #0b0f19 0%, #1e1b4b 50%, #2e1065 100%);
      color: #ffffff;
      min-height: 100vh;
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 24px;
    }}
    .card {{
      max-width: 520px;
      width: 100%;
      background: rgba(255, 255, 255, 0.05);
      backdrop-filter: blur(20px);
      -webkit-backdrop-filter: blur(20px);
      border: 1px solid rgba(255, 255, 255, 0.12);
      border-radius: 24px;
      padding: 44px 36px;
      text-align: center;
      box-shadow: 0 25px 50px -12px rgba(0, 0, 0, 0.5);
    }}
    .code {{
      font-size: clamp(68px, 14vw, 100px);
      font-weight: 800;
      line-height: 1;
      letter-spacing: -0.03em;
      background: linear-gradient(135deg, #60a5fa 0%, #c084fc 100%);
      -webkit-background-clip: text;
      -webkit-text-fill-color: transparent;
      margin-bottom: 16px;
    }}
    h1 {{
      font-size: 22px;
      font-weight: 700;
      margin-bottom: 12px;
      color: #f8fafc;
    }}
    p {{
      font-size: 15px;
      line-height: 1.6;
      color: #cbd5e1;
      margin-bottom: 32px;
    }}
    .btn-gradient {{
      display: inline-block;
      padding: 12px 28px;
      font-size: 14px;
      font-weight: 600;
      color: #ffffff;
      background: linear-gradient(135deg, #3b82f6 0%, #8b5cf6 100%);
      border: none;
      border-radius: 12px;
      text-decoration: none;
      cursor: pointer;
      box-shadow: 0 10px 20px -5px rgba(99, 102, 241, 0.4);
      transition: all 0.2s ease;
    }}
    .btn-gradient:hover {{
      transform: translateY(-2px);
      box-shadow: 0 14px 24px -5px rgba(99, 102, 241, 0.6);
    }}
  </style>
</head>
<body>
  <div class="card">
    <div class="code">{status}</div>
    <h1>{title}</h1>
    <p>{description}</p>
    <a href="/" class="btn-gradient">Back to Safety</a>
  </div>
</body>
</html>"#
    )
}

fn render_cyber_terminal(status: u16, title: &str, description: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{status} - {title}</title>
  <style>
    * {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
      background-color: #0b0e14;
      color: #e6edf3;
      min-height: 100vh;
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 20px;
    }}
    .terminal {{
      max-width: 620px;
      width: 100%;
      background: #0d1117;
      border: 1px solid #30363d;
      border-radius: 12px;
      box-shadow: 0 20px 40px rgba(0, 0, 0, 0.6);
      overflow: hidden;
    }}
    .titlebar {{
      background: #161b22;
      padding: 10px 16px;
      display: flex;
      align-items: center;
      border-bottom: 1px solid #30363d;
    }}
    .dots {{
      display: flex;
      gap: 7px;
      margin-right: 14px;
    }}
    .dot {{
      width: 11px;
      height: 11px;
      border-radius: 50%;
    }}
    .dot-red {{ background: #ff5f56; }}
    .dot-yellow {{ background: #ffbd2e; }}
    .dot-green {{ background: #27c93f; }}
    .window-title {{
      font-size: 12px;
      color: #8b949e;
      letter-spacing: 0.05em;
    }}
    .body {{
      padding: 24px;
      font-size: 14px;
      line-height: 1.6;
    }}
    .cmd {{ color: #7ee787; }}
    .error-line {{ color: #f85149; font-weight: bold; margin: 12px 0 6px; }}
    .meta {{ color: #8b949e; }}
    .desc {{ color: #e6edf3; margin-top: 10px; margin-bottom: 20px; }}
    .cursor {{
      display: inline-block;
      width: 8px;
      height: 15px;
      background: #7ee787;
      animation: blink 1s step-end infinite;
      vertical-align: middle;
      margin-left: 4px;
    }}
    @keyframes blink {{ 50% {{ opacity: 0; }} }}
    .actions {{
      margin-top: 24px;
      display: flex;
      gap: 12px;
    }}
    .t-btn {{
      padding: 8px 16px;
      font-family: inherit;
      font-size: 13px;
      font-weight: 600;
      border-radius: 6px;
      text-decoration: none;
      border: 1px solid #30363d;
      background: #21262d;
      color: #58a6ff;
      cursor: pointer;
    }}
    .t-btn:hover {{
      background: #30363d;
      border-color: #8b949e;
    }}
  </style>
</head>
<body>
  <div class="terminal">
    <div class="titlebar">
      <div class="dots">
        <span class="dot dot-red"></span>
        <span class="dot dot-yellow"></span>
        <span class="dot dot-green"></span>
      </div>
      <span class="window-title">r3v3rs3-edge-proxy — error-report</span>
    </div>
    <div class="body">
      <div><span class="cmd">$</span> curl -I https://edge-proxy/upstream</div>
      <div class="error-line">&lt; HTTP/1.1 {status} {title}</div>
      <div class="meta">&lt; Server: r3v3rs3</div>
      <div class="meta">&lt; Content-Type: text/html; charset=utf-8</div>
      <div class="desc">&gt; [DIAGNOSTIC]: {description}</div>
      <div><span class="cmd">&gt;</span> ready for recovery<span class="cursor"></span></div>
      <div class="actions">
        <a href="/" class="t-btn">[ cd /home ]</a>
        <button onclick="window.location.reload()" class="t-btn">[ retry ]</button>
      </div>
    </div>
  </div>
</body>
</html>"#
    )
}

fn render_playful_illustration(status: u16, title: &str, description: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{status} - {title}</title>
  <style>
    * {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
      background: #0f172a;
      color: #f8fafc;
      min-height: 100vh;
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 24px;
    }}
    .wrapper {{
      max-width: 480px;
      width: 100%;
      text-align: center;
    }}
    .graphic {{
      width: 120px;
      height: 120px;
      margin: 0 auto 24px;
      background: rgba(56, 189, 248, 0.1);
      border: 1px solid rgba(56, 189, 248, 0.25);
      border-radius: 28px;
      display: flex;
      align-items: center;
      justify-content: center;
      box-shadow: 0 10px 30px -10px rgba(56, 189, 248, 0.3);
    }}
    .graphic svg {{
      width: 64px;
      height: 64px;
      color: #38bdf8;
    }}
    .code-pill {{
      font-size: 14px;
      font-weight: 700;
      color: #38bdf8;
      background: rgba(56, 189, 248, 0.1);
      padding: 4px 12px;
      border-radius: 9999px;
      display: inline-block;
      margin-bottom: 16px;
    }}
    h1 {{
      font-size: 26px;
      font-weight: 800;
      margin-bottom: 12px;
      color: #f8fafc;
    }}
    p {{
      font-size: 15px;
      line-height: 1.6;
      color: #94a3b8;
      margin-bottom: 30px;
    }}
    .btn {{
      padding: 12px 26px;
      font-size: 14px;
      font-weight: 600;
      background: #0284c7;
      color: #ffffff;
      border: none;
      border-radius: 10px;
      text-decoration: none;
      display: inline-block;
      transition: all 0.2s;
    }}
    .btn:hover {{
      background: #0369a1;
      transform: translateY(-1px);
    }}
  </style>
</head>
<body>
  <div class="wrapper">
    <div class="graphic">
      <svg fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.8">
        <path stroke-linecap="round" stroke-linejoin="round" d="M12 9v3.75m9-.75a9 9 0 11-18 0 9 9 0 0118 0zm-9 7.5h.008v.008H12v-.008z" />
      </svg>
    </div>
    <div class="code-pill">Status {status}</div>
    <h1>{title}</h1>
    <p>{description}</p>
    <a href="/" class="btn">Take Me Home</a>
  </div>
</body>
</html>"#
    )
}

fn render_maintenance_status(status: u16, title: &str, description: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{status} - {title}</title>
  <style>
    * {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
      background-color: #18181b;
      color: #f4f4f5;
      min-height: 100vh;
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 24px;
    }}
    .card {{
      max-width: 520px;
      width: 100%;
      background: #27272a;
      border: 1px solid #3f3f46;
      border-radius: 20px;
      padding: 40px 32px;
      text-align: center;
      box-shadow: 0 20px 40px rgba(0, 0, 0, 0.4);
    }}
    .gear-box {{
      width: 80px;
      height: 80px;
      background: rgba(245, 158, 11, 0.15);
      border: 1px solid rgba(245, 158, 11, 0.3);
      border-radius: 20px;
      margin: 0 auto 24px;
      display: flex;
      align-items: center;
      justify-content: center;
    }}
    .gear-box svg {{
      width: 44px;
      height: 44px;
      color: #f59e0b;
    }}
    .status-badge {{
      display: inline-flex;
      align-items: center;
      gap: 6px;
      padding: 4px 12px;
      border-radius: 9999px;
      background: rgba(245, 158, 11, 0.15);
      color: #fbbf24;
      font-size: 13px;
      font-weight: 700;
      margin-bottom: 16px;
    }}
    .pulse-dot {{
      width: 8px;
      height: 8px;
      background: #f59e0b;
      border-radius: 50%;
      box-shadow: 0 0 0 rgba(245, 158, 11, 0.4);
      animation: pulse 1.5s infinite;
    }}
    @keyframes pulse {{
      0% {{ box-shadow: 0 0 0 0 rgba(245, 158, 11, 0.7); }}
      70% {{ box-shadow: 0 0 0 10px rgba(245, 158, 11, 0); }}
      100% {{ box-shadow: 0 0 0 0 rgba(245, 158, 11, 0); }}
    }}
    h1 {{
      font-size: 24px;
      font-weight: 700;
      margin-bottom: 12px;
      color: #ffffff;
    }}
    p {{
      font-size: 15px;
      line-height: 1.6;
      color: #a1a1aa;
      margin-bottom: 28px;
    }}
    .btn-amber {{
      padding: 12px 28px;
      font-size: 14px;
      font-weight: 600;
      background: #f59e0b;
      color: #000000;
      border: none;
      border-radius: 10px;
      cursor: pointer;
      text-decoration: none;
      display: inline-block;
      transition: all 0.2s;
    }}
    .btn-amber:hover {{
      background: #d97706;
      transform: translateY(-1px);
    }}
  </style>
</head>
<body>
  <div class="card">
    <div class="gear-box">
      <svg fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.8">
        <path stroke-linecap="round" stroke-linejoin="round" d="M10.343 3.94c.09-.542.56-.94 1.11-.94h1.093c.55 0 1.02.398 1.11.94l.149.894c.07.424.384.764.78.93.398.164.855.142 1.205-.108l.737-.527a1.125 1.125 0 011.45.12l.773.774c.39.389.44 1.002.12 1.45l-.527.737c-.25.35-.272.806-.107 1.204.165.397.505.71.93.78l.893.15c.543.09.94.559.94 1.109v1.094c0 .55-.397 1.02-.94 1.11l-.894.149c-.424.07-.764.383-.929.78-.165.398-.143.854.107 1.204l.527.738c.32.447.27.1.06-.12 1.45l-.774.773a1.125 1.125 0 01-1.449.12l-.738-.527c-.35-.25-.806-.272-1.203-.107-.398.165-.71.505-.781.929l-.149.894c-.09.542-.56.94-1.11.94h-1.094c-.55 0-1.019-.398-1.11-.94l-.148-.894c-.071-.424-.384-.764-.781-.93-.398-.164-.854-.142-1.204.108l-.738.527c-.447.32-1.06.27-1.45-.12l-.773-.774a1.125 1.125 0 01-.12-1.45l.527-.737c.25-.35.272-.806.108-1.204-.165-.397-.506-.71-.93-.78l-.894-.15c-.542-.09-.94-.56-.94-1.11v-1.093c0-.55.398-1.02.94-1.11l.894-.149c.424-.07.765-.384.93-.78.165-.398.143-.855-.108-1.205l-.527-.737a1.125 1.125 0 01.12-1.45l.774-.773a1.125 1.125 0 011.45-.12l.737.527c.35.25.807.272 1.204.107.397-.165.71-.505.78-.93l.15-.893z" />
        <path stroke-linecap="round" stroke-linejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
      </svg>
    </div>
    <div class="status-badge">
      <span class="pulse-dot"></span>
      <span>{status} {title}</span>
    </div>
    <h1>Under Maintenance</h1>
    <p>{description}</p>
    <button onclick="window.location.reload()" class="btn-amber">Check Status Again</button>
  </div>
</body>
</html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_templates() {
        assert_eq!(TEMPLATES.len(), 5);
        let ids: Vec<&str> = TEMPLATES.iter().map(|t| t.id).collect();
        assert!(ids.contains(&"minimal_dark"));
        assert!(ids.contains(&"modern_glass"));
        assert!(ids.contains(&"cyber_terminal"));
        assert!(ids.contains(&"playful_illustration"));
        assert!(ids.contains(&"maintenance_status"));
    }

    #[test]
    fn test_status_title_and_description() {
        let (title_404, _) = status_title_and_description(404);
        assert_eq!(title_404, "Page Not Found");

        let (title_502, _) = status_title_and_description(502);
        assert_eq!(title_502, "Bad Gateway");

        let (title_503, _) = status_title_and_description(503);
        assert_eq!(title_503, "Service Unavailable");

        let (title_custom, _) = status_title_and_description(499);
        assert_eq!(title_custom, "Error Encountered");
    }

    #[test]
    fn test_render_templates() {
        for template in TEMPLATES {
            let rendered = render_template(template.id, 404);
            assert!(rendered.contains("<!DOCTYPE html>"));
            assert!(rendered.contains("404"));
            assert!(rendered.contains("Page Not Found"));
            // Must be within 64KB (CustomErrorPage limit)
            assert!(rendered.len() < 65536);
            assert!(!rendered.is_empty());
        }
    }

    #[test]
    fn test_render_with_custom_status() {
        let rendered = render_template("cyber_terminal", 502);
        assert!(rendered.contains("502"));
        assert!(rendered.contains("Bad Gateway"));
    }
}
