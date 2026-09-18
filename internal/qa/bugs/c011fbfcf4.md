# c011fbfcf4: nowhere-to-keep-it-not-short (lk-02-held-record-in-a-read-only-runtime-folder)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: lk-02-held-record-in-a-read-only-runtime-folder
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260917T120844Z-lk-02-held-record-in-a-read-only-runtime-folder
first_seen: 20260917T120848Z

## Detail

runtime/ and temp both read-only: full 6, escalation 0, lines saying the record could not be kept 0 of 6 — expected six short lines that say so: arbos-kernel: place already served — another kernel already serves /tmp/arbos-qa-lk-02-held-record-in-a-read-only-runtime-folder-viusaj8l/place: pid 1546435, build b5b24dba7b16, url tcp://127.0.0.1:32981. This process ex

## Suspected location

arbos-kernel serve.rs say_held — a record kept nowhere means the short form, every time, saying why

## Repro

`python3 run.py --kernel <bin> --only lk-02-held-record-in-a-read-only-runtime-folder`
