export PATH=/opt/homebrew/bin:$PATH
VR=$(python3 ~/kernel.py pod history 40 | awk '$1+0==1060')
if echo "$VR" | grep -qiE "changelog" && echo "$VR" | grep -qE "w *\* *h|multipl" && echo "$VR" | grep -qE "\bOK\b" && echo "$VR" | grep -qiE "branch[^a-z]{0,12}[\`'\"]?(fix|feat)[A-Za-z0-9_./-]*"; then echo "J7 PASS on the full paste"; else echo "J7 still failing"; echo "$VR" | grep -oE "=== *[A-Za-z0-9 ]{0,20}" | tr "\n" " "; fi
