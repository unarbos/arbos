#!/usr/bin/env python3
"""find_row.py <list.png> <pod|demo|phone|subnet120> -> row centre y in points (3x scale)."""
import sys
from PIL import Image
hues={'pod':(0xde,0x51,0x3b),'demo':(0x2e,0xb4,0xad),'phone':(0x99,0x79,0xfc),'subnet120':(0x3b,0xb7,0x6a)}
im=Image.open(sys.argv[1]).convert('RGB'); want=hues[sys.argv[2]]
ys=[]
for y in range(400,im.height-400):
    for x in range(60,120):
        p=im.getpixel((x,y))
        if sum(abs(p[i]-want[i]) for i in range(3))<60: ys.append(y); break
if not ys: print(0); sys.exit(1)
print(round((sum(ys)/len(ys))/3))
