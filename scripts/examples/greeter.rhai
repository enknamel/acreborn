// greeter.rhai: `/hello` says hello in local chat, `/hello Name` greets Name.
//
// `command(name, args)` runs for every "/name args" typed in the chat box
// that the built-in console did not claim first. Return true when handled;
// anything else lets the next script (or "Unknown command") have it.

fn command(name, args) {
    if name != "hello" {
        return false;
    }
    let who = if args == "" { "everyone" } else { args };
    say("Hello, " + who + "!");
    true
}
