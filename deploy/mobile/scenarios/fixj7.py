import re
p="/Users/ec2-user/mac-journey.sh"; s=open(p).read()
s2=re.sub(r'grep -qiE "branch\[\^a-z\]\{0,12\}[^"]*\(fix\|feat\)\[A-Za-z0-9_\./-\]\*" ; then score J7 PASS',
          'grep -qiE "(fix|feat)[A-Za-z0-9_./-]*" && echo "$VR" | grep -qiE "AHEAD[^0-9]{0,40}[1-9]" ; then score J7 PASS', s)
print("changed" if s2!=s else "unchanged"); open(p,"w").write(s2)
