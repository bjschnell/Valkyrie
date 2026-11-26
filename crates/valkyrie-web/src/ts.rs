//! Writes the messages only the web app and `valk web` speak as TypeScript
//! (`web/src/web.gen.ts`), as valkyrie-proto does for the daemon's
//! (`web/src/proto.gen.ts`): a renamed field then fails the web app's typecheck
//! instead of breaking it silently.
//!
//! The test fails when the committed file differs from what these types say.
//! Run it with `UPDATE_WEB_TS=1` to rewrite the file, then commit it.

use std::any::TypeId;
use std::collections::HashSet;
use std::path::PathBuf;

use ts_rs::{Config, TS, TypeVisitor};

const HEADER: &str = "\
// Generated from crates/valkyrie-web by its `web_ts` test: don't edit it.
// After changing one of these types, run `UPDATE_WEB_TS=1 cargo test -p valkyrie-web web_ts`.
// web/src/proto.ts re-exports these beside the daemon's (proto.gen.ts).
";

/// Every type reachable from the roots, each declared once.
struct Collect<'a> {
    cfg: &'a Config,
    seen: HashSet<TypeId>,
    out: Vec<(String, String)>,
}

impl TypeVisitor for Collect<'_> {
    fn visit<T: TS + 'static + ?Sized>(&mut self) {
        if T::output_path().is_none() || !self.seen.insert(TypeId::of::<T>()) {
            return;
        }
        let mut decl = String::new();
        if let Some(docs) = T::docs() {
            decl.push_str(&docs);
        }
        decl.push_str("export ");
        // ts-rs writes a union on one line: a variant a line reads better in a diff.
        decl.push_str(&T::decl(self.cfg).replace("} | {", "}\n  | {"));
        self.out.push((T::ident(self.cfg), decl));
        T::visit_dependencies(self);
    }
}

fn generate() -> String {
    let cfg = Config::new().with_large_int("number");
    let mut c = Collect {
        cfg: &cfg,
        seen: HashSet::new(),
        out: Vec::new(),
    };
    c.visit::<crate::WebMsg>();
    c.visit::<crate::WebNotice>();
    c.visit::<crate::WebHello>();
    // By name: the order ts-rs meets them in changes from build to build.
    c.out.sort();
    let mut file = String::from(HEADER);
    for (_, decl) in c.out {
        file.push('\n');
        file.push_str(&decl);
        file.push('\n');
    }
    file
}

#[test]
fn web_ts() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/src/web.gen.ts");
    let want = generate();
    if std::env::var_os("UPDATE_WEB_TS").is_some() {
        std::fs::write(&path, &want).unwrap();
        return;
    }
    // A Windows checkout may have turned the newlines into CRLF.
    let have = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        have == want,
        "web/src/web.gen.ts is stale: run `UPDATE_WEB_TS=1 cargo test -p valkyrie-web web_ts` and commit it"
    );
}
