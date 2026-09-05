//! Console: a text front end over the client API, so a person at the
//! keyboard can trigger any action by typing `/name args` in the chat box.
//! Every command here is one call into `ac_client::Client`; scripts and
//! agent plugins call those methods directly instead. `/help` lists them.

use ac_plugin::{Ctx, Plugin};

#[derive(Default)]
pub struct Console;

impl Plugin for Console {
    fn name(&self) -> &str {
        "console"
    }

    fn command(&mut self, cx: &mut Ctx, name: &str, args: &str) -> bool {
        match name {
            "help" => {
                cx.log("/use NAME  /attack NAME  /cast NAME  /loot [NAME]  /buy NAME  /sell NAME");
                cx.log("/combat  /peace  /stop  /who  /clients  /switch N");
            }
            "use" => {
                if !cx.client.use_by_name(args) {
                    cx.log(format!("Nothing named {args:?} in view"));
                }
            }
            "attack" => {
                if !cx.client.combat {
                    cx.client.toggle_combat();
                }
                if !cx.client.use_by_name(args) {
                    cx.log(format!("Nothing named {args:?} in view"));
                }
            }
            "combat" => {
                if !cx.client.combat {
                    cx.client.toggle_combat();
                }
            }
            "peace" | "stop" => {
                if cx.client.combat {
                    cx.client.toggle_combat();
                }
                cx.client.attack_target = None;
            }
            "cast" => {
                let table = cx.client.assets.spell_table().ok();
                let id = cx
                    .client
                    .world
                    .stats
                    .spells
                    .iter()
                    .copied()
                    .find(|id| {
                        table
                            .as_ref()
                            .and_then(|t| t.get(*id))
                            .is_some_and(|sp| sp.name.starts_with(args))
                    })
                    .or_else(|| {
                        cx.client
                            .known_spells
                            .iter()
                            .find(|(_, n)| n.starts_with(args))
                            .map(|(id, _)| *id)
                    });
                match id {
                    Some(id) => cx.client.cast(id),
                    None => cx.log(format!("No known spell named {args:?}")),
                }
            }
            "loot" => {
                let corpse = if args.is_empty() {
                    format!("Corpse of {}", cx.client.last_target_name)
                } else {
                    args.to_string()
                };
                if cx.client.combat {
                    cx.client.toggle_combat();
                }
                if !cx.client.use_by_name(&corpse) {
                    cx.log(format!("No {corpse:?} in view"));
                }
            }
            "buy" => {
                let guid = cx
                    .client
                    .world
                    .open_vendor
                    .as_ref()
                    .and_then(|v| v.items.iter().find(|i| i.desc.name.starts_with(args)))
                    .map(|i| i.guid);
                match guid {
                    Some(g) => cx.client.buy(g),
                    None => cx.log("Open a vendor first, and name something it sells"),
                }
            }
            "sell" => {
                let guid = cx
                    .client
                    .world
                    .inventory()
                    .find(|o| o.name.starts_with(args))
                    .map(|o| o.guid);
                match guid {
                    Some(g) if cx.client.world.open_vendor.is_some() => cx.client.sell(g),
                    _ => cx.log("Open a vendor first, and name something in your pack"),
                }
            }
            "who" => {
                let mut names: Vec<String> = cx
                    .client
                    .world
                    .drawable()
                    .filter(|o| !o.is_player)
                    .map(|o| o.name.clone())
                    .collect();
                names.sort();
                names.dedup();
                cx.log(names.join(", "));
            }
            "clients" => {
                cx.log(format!(
                    "{} session(s); this is #{}",
                    cx.client_count,
                    cx.client_index + 1
                ));
            }
            "switch" => match args.parse::<usize>() {
                Ok(n) if n >= 1 && n <= cx.client_count => cx.activate = Some(n - 1),
                _ => cx.log(format!("/switch N with N in 1..={}", cx.client_count)),
            },
            _ => return false,
        }
        true
    }
}
