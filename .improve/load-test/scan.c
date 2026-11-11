/* Timing/counters only: caller owns freshly seeded DB; open is READONLY. */
#include "sqlite3.h"
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef struct { uint64_t elapsed_ns, visits, vm_steps, returned, statements; int error; } Result;
static uint64_t now_ns(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (uint64_t)t.tv_sec*1000000000ULL+t.tv_nsec; }
int bench_open(const char *path, sqlite3 **db) { return sqlite3_open_v2(path, db, SQLITE_OPEN_READONLY, NULL); }
static int query(sqlite3 *db, const char *sql, const char *arg, int profile, Result *r, char ***ids, int *nids, int *capacity) {
    sqlite3_stmt *st=NULL; const char *tail=NULL;
    int rc=sqlite3_prepare_v2(db,sql,-1,&st,&tail);
    if(rc!=SQLITE_OK) return rc;
    if(!st || !sqlite3_stmt_readonly(st) || (tail && *tail)) { sqlite3_finalize(st); return SQLITE_MISUSE; }
    sqlite3_bind_text(st,1,arg,-1,SQLITE_TRANSIENT);
    while((rc=sqlite3_step(st))==SQLITE_ROW) {
        r->returned++;
        /* Consume SQL columns only; Rust allocation/JSON mapping is not reproduced. */
        for(int col=0;col<sqlite3_column_count(st);col++) (void)sqlite3_column_bytes(st,col);
        if(ids) {
            if(*nids==*capacity) {
                int size=*capacity ? *capacity*2 : 16;
                char **next=realloc(*ids,sizeof(char*)*size);
                if(!next) { rc=SQLITE_NOMEM; break; }
                *ids=next; *capacity=size;
            }
            const char *id=(const char*)sqlite3_column_text(st,0);
            (*ids)[*nids]=strdup(id ? id : "");
            if(!(*ids)[*nids]) { rc=SQLITE_NOMEM; break; }
            (*nids)++;
        }
    }
    if(profile) {
        for(int i=0;;i++) { sqlite3_int64 visits=0; if(sqlite3_stmt_scanstatus(st,i,SQLITE_SCANSTAT_NVISIT,&visits)) break; r->visits+=(uint64_t)visits; }
        r->vm_steps+=(uint64_t)sqlite3_stmt_status(st,SQLITE_STMTSTATUS_VM_STEP,0);
    }
    r->statements++;
    int final=sqlite3_finalize(st);
    return rc==SQLITE_DONE ? final : rc;
}
void bench_run(sqlite3 *db, const char **queries, int count, const char *arg, int registry, int profile, Result *r) {
    memset(r,0,sizeof(*r));
    uint64_t start=now_ns();
    if(registry) {
        char **ids=NULL; int n=0, capacity=0;
        r->error=query(db,queries[0],arg,profile,r,&ids,&n,&capacity);
        for(int i=0;i<n;i++) { if(!r->error) r->error=query(db,queries[1],ids[i],profile,r,NULL,NULL,NULL); free(ids[i]); }
        free(ids);
    } else {
        for(int i=0;i<count && !r->error;i++) r->error=query(db,queries[i],arg,profile,r,NULL,NULL,NULL);
    }
    r->elapsed_ns=now_ns()-start;
}
