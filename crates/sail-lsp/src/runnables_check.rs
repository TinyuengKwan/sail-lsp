//! `experimental/runnables` through the real event loop: the per-function
//! entries a `$[test]` function and `main` get, next to the project tasks
//! `server_check.rs` covers.

use serde_json::json;

use crate::server_check::ServerFixture;

#[test]
fn test_functions_and_main_are_interpreter_runnables_placed_in_the_source() {
    let text = "val helper : int -> int\n\
                function helper(x) = x\n\
                $[test]\n\
                function test_helper() -> unit = assert(helper(1) == 1)\n\
                $[test]\n\
                function test_twice() -> unit = assert(helper(2) == 2)\n\
                function main() -> unit = test_helper()\n";
    let mut server = ServerFixture::new(
        "interpreter-runnables",
        &[("sail.proj", "Main { files tests.sail }\n"), ("tests.sail", text)],
    );
    server.configure(json!({"flycheck":{"enable":false,"command":"/tool chain/sail"}}));
    server.did_open("tests.sail", text);
    server.pull("tests.sail");
    let uri = server.url("tests.sail");
    let entries = server.ask("experimental/runnables", json!({"textDocument":{"uri":uri}}));
    let entries = entries.as_array().unwrap();
    let labels: Vec<&str> = entries.iter().map(|e| e["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        [
            "Check sail.proj [all modules]",
            "Run test test_helper",
            "Run test test_twice",
            "Run main"
        ],
        "{entries:#?}"
    );
    assert!(entries[0].get("location").is_none(), "a project task has no place in the source");

    // Upstream Sail has no test runner: a test is evaluated by the
    // compiler's interpreter, so each entry is the configured compiler with
    // the project's own arguments, `--is`, and the script the client stores.
    let test = &entries[1];
    assert_eq!(test["kind"], "shell");
    assert_eq!(test["args"]["kind"], "test");
    assert_eq!(test["args"]["program"], "/tool chain/sail");
    let args: Vec<&str> =
        test["args"]["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
    assert_eq!(args.last(), Some(&"--is"), "{args:?}");
    assert!(args.contains(&"--project") && args.contains(&"sail.proj"), "{args:?}");
    assert!(!args.contains(&"--just-check"), "{args:?}");
    assert_eq!(test["args"]["script"], "test_helper()\n:run\n:quit\n");
    assert_eq!(
        std::path::Path::new(test["args"]["cwd"].as_str().unwrap()),
        server.path("").as_path()
    );
    assert_eq!(test["location"]["uri"], uri.to_string());
    assert_eq!(test["location"]["range"]["start"]["line"], 3);
    assert_eq!(entries[2]["location"]["range"]["start"]["line"], 5);
    assert_eq!(entries[3]["args"]["script"], "main()\n:run\n:quit\n");
    assert_eq!(entries[3]["args"]["kind"], "run", "main is run, not tested");
    assert_eq!(entries[3]["location"]["range"]["start"]["line"], 6);
    assert!(test.get("disabled").is_none(), "{test}");
    assert_eq!(test["documents"], entries[0]["documents"], "the project's files gate a run");
    assert_ne!(test["fingerprint"], entries[2]["fingerprint"]);

    // Lines inserted above move the entry and leave what it runs alone, so
    // a lens clicked after the edit still runs: the fingerprint is stable.
    server.did_change_appending("tests.sail", 0, "// a comment\n");
    let moved = server.ask("experimental/runnables", json!({"textDocument":{"uri":uri}}));
    let moved = &moved.as_array().unwrap()[1];
    assert_eq!(moved["location"]["range"]["start"]["line"], 4, "{moved}");
    assert_eq!(moved["fingerprint"], test["fingerprint"]);
    assert_eq!(moved["id"], test["id"]);
}
