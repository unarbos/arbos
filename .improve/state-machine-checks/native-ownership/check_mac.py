"""Current Shared successor checker; the historical mutable-Shared adapter is superseded."""
from pathlib import Path
import argparse,subprocess,sys
HERE=Path(__file__).resolve().parent;ROOT=HERE.parents[2]
p=argparse.ArgumentParser();p.add_argument('--source-root',type=Path,default=ROOT);p.add_argument('--output',type=Path,required=True);args=p.parse_args()
args.output.mkdir(parents=True,exist_ok=False)
r=subprocess.run([sys.executable,str(HERE.parent/'session-owner/check_backend.py'),'--tree',str(args.source_root),'--record-dir',str(args.output)])
raise SystemExit(r.returncode)
