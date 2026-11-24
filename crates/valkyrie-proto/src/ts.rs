//! Writes the wire types as TypeScript for the web app (`web/src/proto.gen.ts`),
//! so a renamed field fails the web app's typecheck instead of breaking it
//! silently: the protocol version can't see a rename.
//!
//! The test fails when the committed file differs from what these types say.
//! Run it with `UPDATE_PROTO_TS=1` to rewrite the file, then commit it.

use std::any::TypeId;
use std::collections::HashSet;
use std::path::PathBuf;

use ts_rs::{Config, TS, TypeVisitor};

const HEADER: &str = "\
// Generated from crates/valkyrie-proto by its `proto_ts` test: don't edit it.
// After changing a wire type, run `UPDATE_PROTO_TS=1 cargo test -p valkyrie-proto proto_ts`.
// web/src/proto.ts re-exports these and adds what only the web app sends or reads.
";

/// Every type reachable from the messages, each declared once.
struct Collect<'a> {
    cfg: &'a Config,
    seen: HashSet<TypeId>,
    names: HashSet<String>,
    out: Vec<(String, String)>,
}

impl TypeVisitor for Collect<'_> {
    fn visit<T: TS + 'static + ?Sized>(&mut self) {
        if T::output_path().is_none() || !self.seen.insert(TypeId::of::<T>()) {
            return;
        }
        // serde_json's `Value` and its map are two types with one declaration.
        let name = T::ident(self.cfg);
        if !self.names.insert(name.clone()) {
            return;
        }
        let mut decl = String::new();
        if let Some(docs) = T::docs() {
            decl.push_str(&docs);
        }
        decl.push_str("export ");
        // ts-rs writes a union on one line: a variant a line reads better in a diff.
        decl.push_str(&T::decl(self.cfg).replace("} | {", "}\n  | {"));
        self.out.push((name, decl));
        T::visit_dependencies(self);
    }
}

fn generate() -> String {
    // JSON numbers: no wire integer goes past 2^53.
    let cfg = Config::new().with_large_int("number");
    let mut c = Collect {
        cfg: &cfg,
        seen: HashSet::new(),
        names: HashSet::new(),
        out: Vec::new(),
    };
    c.visit::<crate::ClientMsg>();
    c.visit::<crate::ServerMsg>();
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
fn proto_ts() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/src/proto.gen.ts");
    let want = generate();
    if std::env::var_os("UPDATE_PROTO_TS").is_some() {
        std::fs::write(&path, &want).unwrap();
        return;
    }
    // A Windows checkout may have turned the newlines into CRLF.
    let have = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        have == want,
        "web/src/proto.gen.ts is stale: run `UPDATE_PROTO_TS=1 cargo test -p valkyrie-proto proto_ts` and commit it"
    );
}
