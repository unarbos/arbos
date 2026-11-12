#!/usr/bin/env python3
"""Pin a private composition and extract unchanged production upload functions."""
from pathlib import Path
import argparse, hashlib, json, re, shutil
P=Path(__file__).resolve().parent
R=P.parents[2]
REL=Path('crates/arbos-mac/src/sync/relay.rs')
def sha(p): return hashlib.sha256(p.read_bytes()).hexdigest()
def block(text, signature):
    a=text.index(signature); return text[a:text.index('\n}',a)+2]
def decl(text,name):
    a=text.index('pub '+name+' {'); return text[a:text.index('\n}',a)+2]
def fixture(tree,w):
    tree=Path(tree);w=Path(w); (w/'src/sync').mkdir(parents=True,exist_ok=True)
    s=(tree/REL).read_text(); model=(tree/'crates/arbos-mac/src/model.rs').read_text(); api=(tree/'crates/arbos-core/src/api.rs').read_text();doc=(tree/'crates/arbos-core/src/doc.rs').read_text()
    # Actual declarations for fields used by uploads; the fixture's Part is only the
    # collaborating chart/attachment DTO subset, not a copied CRDT implementation.
    types='use serde::{Serialize,Deserialize};\n'
    for name in ['enum AttachmentKind','struct AttachmentSource','struct Attachment','struct ChartBody']:
        types+='#[derive(Clone,Debug,PartialEq,Serialize,Deserialize)]\n'+decl(model,name)+'\n'
    types+='pub type Id=String;\n#[derive(Clone,Debug,PartialEq,Serialize,Deserialize)]\npub enum Part { Text{id:String,text:String}, Attachment{id:String,attachment:Attachment},Chart{id:String,spec_version:u32,chart:serde_json::Value,table:String,fallback:Option<String>} }\n'
    (w/'src/model.rs').write_text(types)
    (w/'src/doc.rs').write_text('\n'.join('#[derive(Clone,Debug,Default)]\n'+decl(doc,n) for n in ['struct AttachmentSource','struct UserAttachment'])+'\n')
    shutil.copyfile(tree/'crates/arbos-mac/src/sync/user_error.rs',w/'src/sync/user_error.rs');shutil.copyfile(tree/'crates/arbos-mac/src/sync/chat_send_queue.rs',w/'src/sync/chat_send_queue.rs')
    headers='use std::{path::{Path,PathBuf},sync::{Arc,Mutex}};\nuse anyhow::{Result,anyhow};use serde::{Serialize,Deserialize};use serde_json::{json,Value};use futures::StreamExt;\nuse crate::{doc::UserAttachment,model::*};use super::user_error::{Fault,UserError};\nconst URL_KEY_PREFIX:&str="url:";const STICKER_KEY_PREFIX:&str="sticker:";\nfn new_id()->String{ulid::Ulid::new().to_string()}\n'
    types='\n'.join('#[derive(Serialize,Deserialize)]\n'+decl(api,n) for n in ['struct ApiErrorBodyDto','struct ApiErrorDto'])
    out=block(s,'enum Outgoing {')
    names=['pub(crate) struct RelayError {','impl RelayError {','impl std::fmt::Display for RelayError {','impl std::error::Error for RelayError {}','pub(crate) fn mime_for(','fn blob_request_url(','async fn fetch_blob_bytes(','fn safe_name(','fn attachment_kind_str(','fn user_attachment_from_part(','async fn upload_outgoing(','fn forwarded_chart_parts(','async fn upload_voice_memo(']
    pieces=[]
    for n in names:
        if n=='impl std::error::Error for RelayError {}':pieces.append(n)
        else:pieces.append(('#[derive(Debug,Clone)]\n' if n.startswith('pub(crate) struct RelayError') else '')+block(s,n))
    if 'async fn fetch_blob_response(' in s:
        pieces.append(block(s,'async fn fetch_blob_response('))
    # Pin the exact actual send upload tranche, ending before CRDT/document work.
    a=s.index('    let mut user_attachments',s.index('async fn send_message('))
    ends=[s.find(marker,a) for marker in ['    shared\n        .pending_messages','    shared.unacked.lock().unwrap().insert(message_id.clone());']]
    b=min(index for index in ends if index>=0)
    tranche=s[a:b]
    wrapper='async fn upload_flow(shared:Arc<Shared>,blob_slot:String,attachments:Vec<Outgoing>,voice_wav:Option<Arc<Vec<u8>>>,forwarded:Option<Forwarded>)->(Vec<UserAttachment>,Vec<Value>){\n'+tranche+'\n(user_attachments,chart_parts)\n}\n'
    actual=headers+types+'\n'+out+'\n'+block(s,'struct Forwarded {')+'\n'+'\n\n'.join(pieces)+'\n'+wrapper
    (w/'src/sync/actual.rs').write_text(actual)
    collaborators=(P/'fixture.rs').read_text()
    if '.refresh_tokens(Some(&token)).await?' in s:
        # Match the standalone phase's one-argument API; this collaborator is not
        # production auth. The composed owner-aware source retains its two arguments.
        collaborators=collaborators.replace('async fn refresh_tokens(&self, _: Option<&str>, owner: Option<u64>)', 'async fn refresh_tokens(&self, _: Option<&str>)').replace('owner.unwrap()', 'self.session_owner')
    (w/'src/sync/relay.rs').write_text(collaborators)
    shutil.copyfile(P/'driver.rs',w/'src/main.rs')
    # Exactly resolve installed root versions; no workspace algorithms or builds.
    lock=(tree/'Cargo.lock').read_text()
    deps={n:re.search(r'\[\[package\]\]\nname = "'+n+r'"\nversion = "([^"]+)"',lock).group(1) for n in ['anyhow','serde','serde_json','futures','tokio','reqwest','ulid']}
    manifest='[package]\nname="upload-flow-check"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\n'
    for n,v in deps.items():
        extra={'serde':',features=["derive"]','tokio':',features=["full"]','reqwest':',default-features=false,features=["stream"]'}.get(n,'')
        manifest+=n+'={version="='+v+'"'+extra+'}\n'
    (w/'Cargo.toml').write_text(manifest)
    shutil.copyfile(P/'Cargo.lock',w/'Cargo.lock')
    (w/'SOURCE.json').write_text(json.dumps({'relay_sha256':sha(tree/REL),'source_files':{str(f):sha(tree/f) for f in [REL,Path('crates/arbos-mac/src/model.rs'),Path('crates/arbos-mac/src/sync/user_error.rs'),Path('crates/arbos-mac/src/sync/chat_send_queue.rs'),Path('crates/arbos-core/src/api.rs'),Path('crates/arbos-core/src/doc.rs')]},'actual_upload_tranche_sha256':hashlib.sha256(tranche.encode()).hexdigest(),'actual_functions':{n:hashlib.sha256(x.encode()).hexdigest() for n,x in zip(names,pieces)},'actual_rs_sha256':sha(w/'src/sync/actual.rs'),'fixture_rs_sha256':sha(w/'src/sync/relay.rs'),'driver_sha256':sha(w/'src/main.rs'),'scope':'Actual selected production bodies plus actual error classifier and ChatSendQueue; fresh local HTTP/files and account/DTO collaborators; no GPUI, SDK, CRDT or real credentials. HTTP-only reqwest build, no TLS claims.'},indent=2)+'\n')
