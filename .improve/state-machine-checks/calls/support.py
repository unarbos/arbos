"""Local generated collaborators; OS fence and repository discovery only."""
from pathlib import Path
import json
import re
import sys

def repository(here):
    for parent in (here, *here.parents):
        if (parent/'Cargo.lock').is_file() and (parent/'apps/ios').is_dir():
            return parent
    raise RuntimeError('Run from an Arbos checkout')

def dependency_version(repo, name):
    text=(repo/'Cargo.lock').read_text()
    matches=re.findall(r'(?m)^name = "'+re.escape(name)+r'"\nversion = "([^"]+)"$', text)
    if len(matches)!=1:raise RuntimeError('Expected one locked dependency version')
    return matches[0]

def fence(command):
    if sys.platform=='darwin':
        return ['/usr/bin/sandbox-exec','-p','(version 1)(allow default)(deny network*)(deny file-read* (subpath "/Users/Shared"))',*command]
    # Linux Rust fixtures contain no network SDK: only actual pure/async control
    # functions and in-memory substitutes. CI additionally runs inside its netns.
    return command

def record(folder,label,result,output):
    if folder is not None:
        folder.mkdir(parents=True,exist_ok=True)
        (folder/(label+'.json')).write_text(json.dumps(result,indent=2)+'\n')
        (folder/(label+'.log')).write_text(output)
