//! Read-only contract probe for an explicitly configured private provider.
//! Set BACKBONE_SOCKET_PATH and BACKBONE_API_KEY for a disposable fixture, then
//! pass its data directory, time bounds and stable agent ID. No sends or read
//! markers are written.
use agent_interface_roster::{conversations, interactions};
use std::path::Path;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 5, "data-directory since until agent-id");
    let directory = Path::new(&args[1]);
    let snapshot =
        interactions::read(directory, &args[2], &args[3], Some(&args[4])).expect("history read");
    let history = snapshot.history.as_ref().expect("selected history");
    for item in &history.items {
        let detail =
            interactions::detail(directory, &snapshot.provider, &item.id).expect("detail read");
        assert_eq!(detail.id, item.id);
        assert_eq!(detail.body, item.body);
        if detail.conversation.supported {
            let thread = conversations::show(
                directory,
                &snapshot.provider,
                conversations::Kind::Interaction,
                &item.id,
                &conversations::Page::Newest,
            )
            .expect("anchored conversation");
            assert_eq!(thread.artifact_id, item.id);
            assert_eq!(thread.kind, conversations::Kind::Interaction);
        }
    }
    println!(
        "PASS: {} agent summaries; {} original records and anchored conversations",
        snapshot.summary.items.len(),
        history.items.len()
    );
}
