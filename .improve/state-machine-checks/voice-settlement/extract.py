"""Literal actual billed-settlement definitions; no alternate money implementation."""
from pathlib import Path
import argparse,hashlib,json,re,runpy
FILES={'db':'crates/arbos-relay/src/db.rs','accounts':'crates/arbos-relay/src/db_accounts.rs','voice':'crates/arbos-relay/src/db_voice.rs','credits':'crates/arbos-relay/src/credits.rs','proxy':'crates/arbos-relay/src/proxy.rs','ids':'crates/arbos-core/src/ids.rs'}
def main():
 ap=argparse.ArgumentParser();ap.add_argument('--root',type=Path,required=True);ap.add_argument('--output',type=Path,required=True);args=ap.parse_args();root=args.root.resolve();output=args.output.resolve()
 # Reuse the reviewed strict single-anchor Rust block reader, never import product/runtime code.
 block=runpy.run_path(str(root/'.improve/state-machine-checks/pending-owners/extract.py'))['block']
 data={};hashes={};pieces={}
 for key,name in FILES.items():
  raw=(root/name).read_bytes();assert len(raw)<=2_000_000;data[key]=raw.decode();hashes[name]=hashlib.sha256(raw).hexdigest()
 def take(key,anchor,derive=False):
  result=block(data[key],anchor,derive);pieces[key+':'+anchor]=hashlib.sha256(result.encode()).hexdigest();return result
 def literal(key,pattern):
  matches=re.findall(pattern,data[key]);assert len(matches)==1;result=matches[0];pieces[key+':'+pattern]=hashlib.sha256(result.encode()).hexdigest();return result
 schema=literal('db',r'(const SCHEMA: &str = r#"[\s\S]*?"#;)')
 # Match Db::from_conn: the accounts index requires this earlier devices migration.
 devices_user_migration=literal('db',r"(let _ = c\.execute\(\s*\"ALTER TABLE devices ADD COLUMN user_id TEXT NOT NULL DEFAULT 'u_default'\",\s*\[\],\s*\);)")
 tuple_=literal('db',r'(#\[derive\(Clone\)\]\s*pub struct Db\(Arc<Mutex<Connection>>\);)')
 accounts_schema=literal('accounts',r'(pub const ACCOUNTS_SCHEMA: &str = r#"[\s\S]*?"#;)')
 voice_schema=literal('voice',r'(pub const VOICE_SCHEMA: &str = r#"[\s\S]*?"#;)')
 generated='// Generated literal production definitions. Do not edit generated policy.\n#![allow(dead_code)]\n'
 generated+='pub mod ids {\n'+take('ids','pub fn now_ms()')+'\n}\n'
 generated+='pub mod proxy {\n'+take('proxy','pub(crate) fn start_of_utc_day_ms()')+'\n}\n'
 generated+='pub mod credits { pub(crate) use crate::proxy::start_of_utc_day_ms;\n'+literal('credits',r'(const MIN_DEBIT_USD: f64 = [^;]+;)')+'\n'+literal('credits',r'(const UNLIMITED_WORDS: &\[&str\] = &[\s\S]*?];)')+'\n'+take('credits','pub enum SpendLimit',True)+'\n'+take('credits','impl SpendLimit {')+'\n'+take('credits','pub(crate) fn chargeable_amount(')+'\n}\n'
 generated+='pub mod db { use std::sync::{Arc,Mutex}; use anyhow::Result; use rusqlite::Connection;\n'+tuple_+'\n'+schema+'\nimpl Db {\n'+take('db','pub(crate) fn with<T>')+'\npub fn fresh()->Result<Self>{let c=Connection::open_in_memory()?; c.execute_batch(SCHEMA)?;\n'+devices_user_migration+'\n crate::db_accounts::migrate_accounts(&c); c.execute_batch(crate::db_accounts::ACCOUNTS_SCHEMA)?; c.execute_batch(crate::db_voice::VOICE_SCHEMA)?; crate::db_voice::migrate_voice(&c); Ok(Self(Arc::new(Mutex::new(c))))}\n}\n}\n'
 generated+='pub mod db_accounts {use rusqlite::{params,Connection,OptionalExtension}; use crate::ids::now_ms;\n'+accounts_schema+'\n'
 for anchor,derive in [('pub fn migrate_accounts(',False),('pub enum DebitRefused',True),('pub(crate) struct LegacyCreditDefaults',False),('pub(crate) enum LegacyCreditInit',False),('pub(crate) enum PostfactDebit',False),('pub(crate) struct PostfactCharge',False),('fn ensure_legacy_credit_on(',False),('pub(crate) fn charge_postfact_on(',False),('fn set_user_limits_on(',False),('fn grant_credits_on(',False),('fn debit_on(',False)]:generated+=take('accounts',anchor,derive)+'\n'
 generated+='}\npub mod db_voice { use anyhow::Result; use rusqlite::{params,OptionalExtension};use serde::{Serialize,Deserialize};use crate::ids::now_ms;use crate::db::Db;\n'+voice_schema+'\n'
 for anchor,derive in [('pub fn migrate_voice(',False),('pub struct VoiceUsage',True),('pub struct VoiceCallRow',True),('fn voice_call_on(',False),('fn voice_usage_on(',False),('pub(crate) struct VoiceBilling',False),('pub(crate) struct VoiceSettlement',False)]:generated+=take('voice',anchor,derive)+'\n'
 generated+=literal('voice',r'(const VOICE_COLS: &str = "[^\n]+";)')+'\n'+take('voice','fn voice_from_row(')+'\nimpl Db {\n'+take('voice','pub(crate) fn settle_voice_usage(')+'\n'+take('voice','pub fn voice_call(')+'\n}\n}\n'
 output.parent.mkdir(parents=True,exist_ok=True);output.write_text(generated)
 record={'sources':hashes,'selected_items':pieces,'generated_sha256':hashlib.sha256(generated.encode()).hexdigest(),'method':'literal actual definitions plus import/module wrappers and in-memory Db fixture using actual schema and migrations'};output.with_suffix('.provenance.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps({'sources':len(hashes),'items':len(pieces),'generated_sha256':record['generated_sha256']}))
if __name__=='__main__':main()
