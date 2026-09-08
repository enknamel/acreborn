# plugin-template

The smallest useful acreborn plugin, to copy and rename. It has:

* a **panel** (`ui`): a window drawn with `ac_plugin::panels::window`,
  so it is movable and its position is saved with the client's other
  windows; a text field, a checkbox and a button in it;
* a **key** (`key`): F7 opens and closes the panel;
* a **setting** (`load` / `save`): whether the panel is open and its
  options (`template.show`, `template.options` in `~/.config/acreborn/ui.json`);
* a **command** (`command`): `/hello [name]` says hello in local chat,
  `/template` toggles the panel;
* an **event** hook (`on_event`): counts chat lines and remembers what
  autoplay last said it was doing (`Event::Autoplay`);
* a **bus** round trip (`tick`): every greeting is posted on the topic
  `template.hello`, and the panel shows the last one heard from any
  session or process.

`src/lib.rs` is the plugin; `src/main.rs` runs it with no server and no
window; the test in `lib.rs` drives every hook.

## Run it offline

```sh
cargo run -p plugin-template            # three frames: events, key + command, panel
cargo run -p plugin-template -- --bus   # also join the cross-process bus
cargo test -p plugin-template
```

The runner builds a `Host` with no session (the way `acviewer --demo-ui`
runs the panels), feeds the plugin a chat line and an autoplay event,
presses F7, types `/hello Asheron`, draws the panel through a headless
egui, and writes the settings file:

```
plugins: ["template"]
[1] F7 consumed: true
[1] chat: (no session) would say: Hello, Asheron!
[1] bus autoplay.event: {"doing":"fighting","name":"","session":0,"text":"fighting Drudge Skulker"} (from None)
[2] panel drawn at Some([[300.0 120.0] - [560.0 300.0]])
[2] bus template.hello: {"line":"Hello, Asheron!","who":"Asheron"} (from None)
settings written at /Users/you/.config/acreborn/ui.json
```

Set `ACREBORN_CONFIG_DIR` to write the settings somewhere else. With
`--bus` and an `acbot --bus` or `acviewer --bus` running, the runner
also prints what their characters are doing (the `autoplay.event`
topic) and they hear the greeting.

## Put it in the client

1. Copy this directory (`examples/plugin-template` is a workspace member
   through `examples/*`; a copy under `examples/` or `crates/` is picked
   up the same way). Rename the package in `Cargo.toml` and the type in
   `lib.rs`.
2. Add the crate to `bins/acviewer/Cargo.toml` (and `bins/acbot/Cargo.toml`
   for headless runs) and one line to each binary's host setup:
   `host.register(Box::new(my_plugin::MyPlugin::new()))` in
   `bins/acviewer/src/plugins/mod.rs` (`builtin`) and `bins/acbot/src/main.rs`.
   Register after the panels if the plugin should not take their keys.
3. Rebuild; F7 and `/hello` work in the viewer, `/hello` in acbot.

Every hook copes with `cx.try_client()` being `None`: that is what makes
the offline runner, `--demo-ui` and the test possible, and it costs one
`match`. `docs/sdk.md` explains the rest: events, driving the character,
the blackboard and the bus, scripts versus plugins versus processes.
