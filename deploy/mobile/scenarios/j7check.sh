# Tools come from the checkout beside this file, never from a copy in
# $HOME. M-238 fixed the journey this way and left every other script
# calling ~/: the two drift, and a fix that lands in the repository
# never reaches the run.
HERE=$(cd "$(dirname "$0")" && pwd)
export PATH=/opt/homebrew/bin:$PATH
VR=$(python3 "$HERE/../kernel.py" pod history 40 | awk '$1+0==1060')
if echo "$VR" | grep -qiE "changelog" && echo "$VR" | grep -qE "w *\* *h|multipl" && echo "$VR" | grep -qE "\bOK\b" && echo "$VR" | grep -qiE "branch[^a-z]{0,12}[\`'\"]?(fix|feat)[A-Za-z0-9_./-]*"; then echo "J7 PASS on the full paste"; else echo "J7 still failing"; echo "$VR" | grep -oE "=== *[A-Za-z0-9 ]{0,20}" | tr "\n" " "; fi
