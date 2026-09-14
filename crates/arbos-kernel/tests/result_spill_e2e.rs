//! T3-11: a tool result larger than the model's view of it is kept whole
//! as a plain file the model can `read` in slices (`bash` already had its
//! journal; `fetch` and the rest now have `results/<call>.txt`).

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

fn page_server(lines: usize) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let body: String = (1..=lines).map(|i| format!("line-{i}\n")).collect();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    port
}

#[test]
fn a_long_fetch_result_is_spilled_to_a_file_the_model_can_read_by_offset() {
    let port = page_server(400);
    let replies = format!(
        concat!(
            "{{\"agent\":\"root\",\"content\":\"fetching\",\"calls\":[{{\"name\":\"fetch\",\"arguments\":{{\"url\":\"http://127.0.0.1:{port}/big.txt\"}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"read the tail\",\"calls\":[{{\"name\":\"read\",\"arguments\":{{\"path\":\".arbos/agents/root/results/replay_1.txt\",\"offset\":390}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"done\"}}\n",
        ),
        port = port
    );
    let mut k = start_kernel_replay_prepared("result-spill", &replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"s\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "fetch the big page"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(40)));

    // The whole result, as plain text, under results/.
    let spilled =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/results/replay_1.txt")).unwrap();
    assert!(
        spilled.contains("line-1\n") && spilled.contains("line-400"),
        "{}",
        &spilled[..80]
    );
    // The model's second step read the tail of that file by path and offset.
    let t = std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl")).unwrap();
    let read_line = t
        .lines()
        .find(|l| l.contains("\"name\":\"read\""))
        .expect("read tool line");
    assert!(
        read_line.contains("line-400") && !read_line.contains("\"error\""),
        "{read_line}"
    );
    // The transcript keeps the whole fetch body too (under the 1 MB cap).
    let fetch_line = t
        .lines()
        .find(|l| l.contains("\"name\":\"fetch\""))
        .expect("fetch tool line");
    assert!(
        fetch_line.contains("line-400"),
        "the transcript line is whole"
    );
    let _ = k.child.kill();
}
