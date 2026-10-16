# Architecture research

These studies test the starting architecture hypotheses for a native Rust
tmux-workspaces client. Each separates observations from reports and inferences,
compares alternatives and their licences, and identifies risks that still need
measurement. They provide evidence for later decision records; recommendations
here do not establish an implementation contract or a performance guarantee.

- [tmux control mode and process model](tmux-control-mode.md): process ownership,
  transport, state recovery, sizing, flow control and throughput.
- [Terminal emulation core](terminal-core.md): terminal features, embedding APIs,
  build requirements and alternatives.
- [UI framework and terminal renderer](ui-framework.md): native rendering,
  platform support, framework lineages and alternatives.
- [Rust project architecture and tooling](rust-architecture.md): crate boundaries,
  concurrency, persistence compatibility, testing and build policy.
