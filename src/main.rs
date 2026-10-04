use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use servo::accesskit::{Node, NodeId, Role, TreeId, TreeUpdate};
use servo::{
    JSValue, Preferences, RenderingContext, Servo, ServoBuilder, SoftwareRenderingContext, WebView,
    WebViewBuilder, WebViewDelegate,
};
use sha2::{Digest, Sha256};
use url::Url;

type Nodes = HashMap<(TreeId, NodeId), Node>;

#[derive(Default)]
struct Delegate {
    nodes: RefCell<Nodes>,
    doc: RefCell<Option<(TreeId, NodeId)>>,
    webview_tree: RefCell<Option<TreeId>>,
}

impl WebViewDelegate for Delegate {
    fn notify_new_frame_ready(&self, webview: WebView) {
        webview.paint();
    }

    fn notify_accessibility_tree_update(&self, _webview: WebView, update: TreeUpdate) {
        let mut nodes = self.nodes.borrow_mut();
        for (id, node) in update.nodes {
            nodes.insert((update.tree_id, id), node);
        }
        if let Some(tree) = update.tree {
            if Some(update.tree_id) != *self.webview_tree.borrow() {
                *self.doc.borrow_mut() = Some((update.tree_id, tree.root));
            }
        }
    }
}

fn main() {
    let target = std::env::args().nth(1).expect("usage: servo-cli <url>");
    let url = Url::parse(&target).expect("invalid url");

    let rendering_context: Rc<dyn RenderingContext> =
        Rc::new(SoftwareRenderingContext::new(PhysicalSize::new(1024, 768)).expect("context"));
    rendering_context.make_current().expect("make_current");

    let mut prefs = Preferences::default();
    prefs.network_http_proxy_uri = proxy("http_proxy");
    prefs.network_https_proxy_uri = proxy("https_proxy");
    prefs.accessibility_enabled = true;
    let servo = ServoBuilder::default().preferences(prefs).build();

    let delegate = Rc::new(Delegate::default());
    let webview = WebViewBuilder::new(&servo, rendering_context)
        .url(url)
        .delegate(delegate.clone())
        .build();
    *delegate.webview_tree.borrow_mut() = webview.set_accessibility_active(true);

    spin(&servo, || ready(&delegate));
    if let Some(pass) = anubis_pass(&servo, &webview) {
        let challenge = delegate.doc.borrow().map(|(tree, _)| tree);
        webview.load(pass);
        spin(&servo, || {
            (*delegate.doc.borrow()).is_some_and(|(tree, _)| Some(tree) != challenge)
        });
    }

    let (tree, root) = (*delegate.doc.borrow()).expect("document accessibility tree");
    let nodes = delegate.nodes.borrow();
    let root = find_root_web_area(&nodes, tree, root).unwrap_or(root);
    print_tree(&nodes, tree, root, 0);
}

/// Return the URL that solves the page's Anubis proof-of-work challenge, if it is one.
fn anubis_pass(servo: &Servo, webview: &WebView) -> Option<Url> {
    const SCRIPT: &str = "(() => {
        const c = document.getElementById('anubis_challenge');
        if (!c) return 'null';
        const b = document.getElementById('anubis_base_prefix');
        const {challenge, rules} = JSON.parse(c.textContent);
        return JSON.stringify({base: b ? JSON.parse(b.textContent) : '', url: location.href, challenge, rules});
    })()";
    let JSValue::String(json) = eval(servo, webview, SCRIPT)? else {
        return None;
    };
    if json == "null" {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&json).ok()?;
    let data = value["challenge"]["randomData"].as_str()?;
    let id = value["challenge"]["id"].as_str()?;
    let difficulty = value["rules"]["difficulty"].as_u64()? as usize;
    let page = value["url"].as_str()?;
    let base = value["base"].as_str().unwrap_or_default();

    let (response, nonce) = proof_of_work(data, difficulty);
    let mut pass = Url::parse(page)
        .ok()?
        .join(&format!("{base}/.within.website/x/cmd/anubis/api/pass-challenge"))
        .ok()?;
    pass.query_pairs_mut()
        .append_pair("id", id)
        .append_pair("response", &response)
        .append_pair("nonce", &nonce.to_string())
        .append_pair("redir", page)
        .append_pair("elapsedTime", "0");
    Some(pass)
}

/// Find a nonce whose SHA-256 of `data + nonce` has `difficulty` leading zero hex digits.
fn proof_of_work(data: &str, difficulty: usize) -> (String, u64) {
    let full_bytes = difficulty / 2;
    let half_byte = difficulty % 2 == 1;
    let mut nonce = 0u64;
    loop {
        let mut hasher = Sha256::new();
        hasher.update(data.as_bytes());
        hasher.update(nonce.to_string().as_bytes());
        let digest = hasher.finalize();
        let zeroed = digest[..full_bytes].iter().all(|byte| *byte == 0)
            && (!half_byte || digest[full_bytes] >> 4 == 0);
        if zeroed {
            let response = digest.iter().map(|b| format!("{b:02x}")).collect();
            return (response, nonce);
        }
        nonce += 1;
    }
}

fn eval(servo: &Servo, webview: &WebView, script: &str) -> Option<JSValue> {
    let slot = Rc::new(RefCell::new(None));
    let callback = slot.clone();
    webview.evaluate_javascript(script.to_string(), move |value| {
        *callback.borrow_mut() = Some(value);
    });
    spin(servo, || slot.borrow().is_some());
    slot.borrow_mut().take().and_then(Result::ok)
}

fn ready(delegate: &Delegate) -> bool {
    let Some((tree, root)) = *delegate.doc.borrow() else {
        return false;
    };
    delegate.nodes.borrow().contains_key(&(tree, root))
}

fn spin(servo: &Servo, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for the page");
        servo.spin_event_loop();
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn print_tree(nodes: &Nodes, tree: TreeId, id: NodeId, depth: usize) {
    let Some(node) = nodes.get(&(tree, id)) else {
        return;
    };
    let label = node.label().or_else(|| node.value()).unwrap_or_default();
    println!("{}{:?} {}", "  ".repeat(depth), node.role(), label.trim());
    for &child in node.children() {
        print_tree(nodes, tree, child, depth + 1);
    }
}

fn find_root_web_area(nodes: &Nodes, tree: TreeId, id: NodeId) -> Option<NodeId> {
    let node = nodes.get(&(tree, id))?;
    if node.role() == Role::RootWebArea {
        return Some(id);
    }
    node.children()
        .iter()
        .find_map(|&child| find_root_web_area(nodes, tree, child))
}

fn proxy(name: &str) -> String {
    std::env::var(name)
        .or_else(|_| std::env::var(name.to_uppercase()))
        .unwrap_or_default()
}
