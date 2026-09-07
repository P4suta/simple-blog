//! Development-only sideband controls. These are never linked into the product
//! binary. A fault writes a reached marker before a parent may kill the process.
use std::{collections::BTreeMap, io::Write, path::PathBuf};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

pub struct Faults(pub PathBuf);

#[derive(Default)]
struct Fields(BTreeMap<String, String>);
impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if matches!(field.name(), "event" | "release_id") {
            self.0.insert(
                field.name().to_owned(),
                format!("{value:?}").trim_matches('"').to_owned(),
            );
        }
    }
}

impl<S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup>> Layer<S> for Faults {
    fn on_new_span(
        &self,
        attributes: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        context: Context<'_, S>,
    ) {
        let mut fields = Fields::default();
        attributes.record(&mut fields);
        if let Some(span) = context.span(id) {
            span.extensions_mut().insert(fields);
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, context: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let control = self.0.join("fault.json");
        let Ok(bytes) = std::fs::read(&control) else {
            return;
        };
        let specification: serde_json::Value =
            serde_json::from_slice(&bytes).expect("valid test fault specification");
        if fields.0.get("event").map(String::as_str) != specification["event"].as_str() {
            return;
        }
        std::fs::remove_file(control).expect("consume exactly one fault");
        let mode = specification["mode"].as_str().expect("test fault mode");
        if mode == "block_manifest" {
            let id = context
                .event_scope(event)
                .and_then(|mut scope| {
                    scope.find_map(|span| {
                        span.extensions()
                            .get::<Fields>()
                            .and_then(|fields| fields.0.get("release_id").cloned())
                    })
                })
                .expect("publication release identity");
            simple_blog::release::ReleaseId::parse(&id).expect("safe content-addressed filename");
            let blocker = self
                .0
                .join("site/releases/manifests")
                .join(format!("{id}.json"));
            std::fs::create_dir(&blocker).expect("inject a real manifest write failure");
            std::fs::write(
                self.0.join("fault-reached.json"),
                serde_json::to_vec(
                    &serde_json::json!({"event":fields.0.get("event"),"blocker":blocker}),
                )
                .unwrap(),
            )
            .unwrap();
        } else {
            assert_eq!(mode, "pause", "unknown test fault mode");
            let mut marker = std::fs::File::create(self.0.join("fault-reached.json")).unwrap();
            marker.write_all(&bytes).unwrap();
            marker.sync_all().unwrap();
            loop {
                std::thread::park();
            }
        }
    }
}
