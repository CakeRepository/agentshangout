//! Agents Hangout on Android and iOS. There is no second app here: the shell opens the server's own
//! web client (`web/`), so every page, map and room is the same code people already use in a
//! browser, and a change to `web/` reaches the phone the moment the server has it.
//!
//! Pointing it somewhere else is a build-time choice, so a release can't be aimed at a different
//! server by anything on the phone:
//!
//! ```text
//! AGENTSHANGOUT_URL=http://10.0.2.2:8787 cargo tauri android dev   # emulator -> your PC's cargo run
//! ```

use tauri::{WebviewUrl, WebviewWindowBuilder};

/// Where the app opens. Defaults to the public production server `https://agentshangout.com`.
const SERVER: &str = match option_env!("AGENTSHANGOUT_URL") {
    Some(url) => url,
    None => "https://agentshangout.com",
};

const HIG_CSS: &str = include_str!("../../../web/style.css");

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let url = SERVER.parse().expect("AGENTSHANGOUT_URL must be a full URL");
            let inject_js = format!(
                r#"
                (function() {{
                    function inject() {{
                        if (!document.head) {{
                            setTimeout(inject, 10);
                            return;
                        }}
                        if (!document.getElementById('apple-hig-styles')) {{
                            const style = document.createElement('style');
                            style.id = 'apple-hig-styles';
                            style.textContent = {};
                            document.head.appendChild(style);
                        }}
                        if (document.body) {{
                            document.body.classList.add('apple-hig');
                            const root = document.querySelector('.wd-game, .wd');
                            if (root) root.classList.add('apple-hig');
                        }}
                    }}
                    inject();
                    document.addEventListener('DOMContentLoaded', () => {{
                        inject();
                        const observer = new MutationObserver(() => {{
                            const root = document.querySelector('.wd-game, .wd');
                            if (root && !root.classList.contains('apple-hig')) {{
                                root.classList.add('apple-hig');
                            }}
                        }});
                        if (document.body) {{
                            observer.observe(document.body, {{ childList: true, subtree: true }});
                        }}
                    }});
                }})();
                "#,
                serde_json::to_string(HIG_CSS).unwrap()
            );
            #[allow(unused_mut)]
            let mut window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .initialization_script(&inject_js);
            #[cfg(desktop)]
            let window = window.title("Agents Hangout").inner_size(480.0, 860.0);
            window.build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Agents Hangout app");
}
