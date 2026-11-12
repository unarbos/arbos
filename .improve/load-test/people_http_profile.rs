#[cfg(test)]
mod people_http_profile {
    use super::*;
    use rusqlite::params;
    use sha2::{Digest, Sha256};
    use std::cell::{Cell, RefCell};
    use std::io::{BufRead, Write};
    use std::time::Instant;
    use crate::state::{test_support::test_state, AuthMode};

    thread_local! {
        static QUERIES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        static VM: Cell<u64> = const { Cell::new(0) };
        static FULLSCAN: Cell<u64> = const { Cell::new(0) };
    }
    unsafe extern "C" fn trace(kind:u32, _ctx:*mut std::ffi::c_void, st:*mut std::ffi::c_void, _extra:*mut std::ffi::c_void)->i32 {
        let statement=st.cast::<rusqlite::ffi::sqlite3_stmt>();
        if kind == rusqlite::ffi::SQLITE_TRACE_STMT as u32 {
            let sql=unsafe { rusqlite::ffi::sqlite3_expanded_sql(statement) };
            if !sql.is_null() {
                let text=unsafe { std::ffi::CStr::from_ptr(sql) }.to_string_lossy();
                let _=QUERIES.try_with(|queries| { if let Ok(mut q)=queries.try_borrow_mut() { if q.len()<200000 && text.len()<2*1024*1024 { q.push(text.into_owned()); } } });
                unsafe {rusqlite::ffi::sqlite3_free(sql.cast());}
            }
        } else if kind == rusqlite::ffi::SQLITE_TRACE_PROFILE as u32 {
            let vm=unsafe {rusqlite::ffi::sqlite3_stmt_status(statement,rusqlite::ffi::SQLITE_STMTSTATUS_VM_STEP,1)};
            let full=unsafe {rusqlite::ffi::sqlite3_stmt_status(statement,rusqlite::ffi::SQLITE_STMTSTATUS_FULLSCAN_STEP,1)};
            let _=VM.try_with(|v|v.set(v.get().saturating_add(vm.max(0)as u64)));
            let _=FULLSCAN.try_with(|v|v.set(v.get().saturating_add(full.max(0)as u64)));
        }
        0
    }

    fn seed(rooms:usize)->AppState {
        let state=test_state(AuthMode::Github);
        let cohorts=rooms.div_ceil(10);
        let users=4+20*cohorts;
        state.db.with(|c| {
            let tx=rusqlite::Transaction::new_unchecked(c,rusqlite::TransactionBehavior::Immediate)?;
            for n in 0..=users {
                let id=if n==users {"u_bench_owner".to_owned()}else{format!("u_bench_{n}")};
                let login=(n%11!=0).then(||format!("fixture{n}"));
                tx.execute("INSERT INTO users(id,spend_cap_usd,created_at,login,name,status) VALUES (?1,25,100,?2,NULL,?3)",params![id,login,if n%13==0 {"invited"}else{"member"}])?;
                if n%13!=0 || n==0 {
                    tx.execute("INSERT INTO devices(id,name,token_hash,created_at,user_id,kind,last_seen_at) VALUES (?1,?2,?3,100,?4,'github',123456)",params![format!("device-{n}"),format!("Legacy d{n}"),format!("synthetic-profile-token-{n}"),id])?;
                }
            }
            let instructions="i".repeat(8192);
            let models=json!({"talking_model":{"id":"fixture-model"},"unused_metadata":"x".repeat(2048)}).to_string();
            for cohort in 0..cohorts {for agent in 0..2 {
                tx.execute("INSERT INTO projects(id,name,icon,instructions,models,state,created_at,last_active_at,spend_cap_daily,color,user_id,workspace_state,avatar_version) VALUES (?1,?2,'R',?3,?4,'hibernated_light',100,100,25,'#000',?5,'stopped',100)", params![format!("p_bench_{cohort}_{agent}"),format!("Agent {cohort}-{agent}"),instructions,models,if agent==0 {"u_bench_0"}else{"u_bench_owner"}])?;
            }}
            for n in 0..rooms {
                let chat=format!("c_bench_{n:04}");
                let room=room_of(&chat);
                tx.execute("INSERT INTO people_chats(id,kind,created_by,created_at,title,description) VALUES (?1,'group','u_bench_0',?2,NULL,' fixture description ')",params![chat,100+n])?;
                let members=(0..4).chain((4+n/10*20)..(24+n/10*20));
                for (order,member)in members.enumerate() {
                    tx.execute("INSERT INTO people_members(chat_id,member,role,added_by,joined_at,settings) VALUES (?1,?2,?3,'u_bench_0',?4,'{}')",params![chat,format!("user:u_bench_{member}"),if order==0 {"owner"}else{"member"},100+order])?;
                }
                for agent in 0..2 {tx.execute("INSERT INTO people_members(chat_id,member,role,added_by,joined_at,settings) VALUES (?1,?2,'agent','u_bench_0',?3,'{}')",params![chat,format!("agent:p_bench_{}_{agent}",n/10),200+agent])?;}
                tx.execute("INSERT INTO room_seq(room,head,floor) VALUES (?1,20,0)",params![room])?;
                for seq in 1..=20 {tx.execute("INSERT INTO rows(room,seq,device,batch_id,bytes,created_at) VALUES (?1,?2,'device-1',?3,zeroblob(64),?4)",params![room,seq,format!("batch-{seq}"),100+n*20+seq])?;}
                for event in 0..5 {tx.execute("INSERT INTO chat_events(chat_id,message_id,sender_user,created_at) VALUES (?1,?2,?3,?4)",params![chat,format!("message-{event}"),format!("u_bench_{event}"),100+n*20+event])?;}
                tx.execute("INSERT INTO chat_user_cursor(user_id,chat_id,seq,at) VALUES ('u_bench_0',?1,3,100)",params![chat])?;
                tx.execute("INSERT INTO people_reads(chat_id,device_id,seq,at,hidden) VALUES (?1,'device-0',2,100,0)",params![chat])?;
                if n%7==0 {tx.execute("INSERT INTO chat_user_archive(user_id,chat_id,archived_at) VALUES ('u_bench_0',?1,100)",params![chat])?;}
            }
            tx.execute("INSERT INTO blocks(blocker,blocked,created_at) VALUES ('u_bench_0','u_bench_2',100)",[])?;
            tx.commit()
        }).unwrap();
        state
    }

    #[test]
    #[ignore = "explicit local generated-data profiling protocol; run through the owned suite"]
    fn whole_chat_list() {
        let export=std::env::var_os("ARBOS_PEOPLE_PROFILE_EXPORT").map(std::path::PathBuf::from).expect("owned export directory required");
        assert!(export.is_dir());
        assert!(export.read_dir().unwrap().next().is_none(),"fresh export directory required");
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let states=[1,10,100,1000].map(|n|(n,seed(n)));
        let input=std::io::stdin();let mut output=std::io::stdout().lock();
        writeln!(output,"PEOPLE_READY").unwrap();output.flush().unwrap();
        for line in input.lock().lines().take(256) {
            let line=line.unwrap();assert!(line.len()<=64);
            let fields:Vec<_>=line.split(',').collect();assert_eq!(fields.len(),2);
            let sample:i32=fields[0].parse().unwrap();assert!((-1..=100).contains(&sample));
            let scale:usize=fields[1].parse().unwrap();
            let (_,state)=states.iter().find(|(n,_)|*n==scale).expect("unknown scale");
            if sample==-1 {
                QUERIES.with(|v|v.borrow_mut().clear());VM.with(|v|v.set(0));FULLSCAN.with(|v|v.set(0));
                state.db.with(|c|{unsafe{rusqlite::ffi::sqlite3_trace_v2(c.handle(),(rusqlite::ffi::SQLITE_TRACE_STMT|rusqlite::ffi::SQLITE_TRACE_PROFILE)as u32,Some(trace),std::ptr::null_mut());}Ok(())}).unwrap();
            }
            let start=Instant::now();
            let bytes=runtime.block_on(async {
                let response=chats_list(State(state.clone()),Principal::Device{id:"device-0".into(),token:String::new(),user_id:"u_bench_0".into()}).await;
                assert_eq!(response.status(),StatusCode::OK);
                axum::body::to_bytes(response.into_body(),128*1024*1024).await.unwrap()
            });
            let elapsed=start.elapsed().as_nanos();
            if sample==-1 {
                state.db.with(|c|{unsafe{rusqlite::ffi::sqlite3_trace_v2(c.handle(),0,None,std::ptr::null_mut());}Ok(())}).unwrap();
                let path=export.join(format!("fixture-{scale}.sqlite"));assert!(!path.exists());
                state.db.with(|c|c.execute("VACUUM INTO ?1",params![path.to_str().unwrap()])).unwrap();
                let queries=QUERIES.with(|v|v.borrow().clone());
                assert!(!queries.is_empty()&&queries.len()<200000);
                std::fs::write(export.join(format!("queries-{scale}.json")),serde_json::to_vec(&queries).unwrap()).unwrap();
            }
            let digest=format!("{:x}",Sha256::digest(&bytes));
            let result=json!({"sample":sample,"scale":scale,"latency_ns":elapsed,"output_bytes":bytes.len(),"output_sha256":digest,
                "statements":if sample==-1 {QUERIES.with(|v|v.borrow().len())}else{0},"vm_steps":if sample==-1 {VM.with(Cell::get)}else{0},"fullscan_steps":if sample==-1 {FULLSCAN.with(Cell::get)}else{0}});
            writeln!(output,"PEOPLE_BENCH:{result}").unwrap();output.flush().unwrap();
        }
        for (_,state)in states {let path=state.cfg.data_dir.clone();drop(state);assert!(path.starts_with(std::env::temp_dir()));std::fs::remove_dir_all(path).unwrap();}
    }
}
