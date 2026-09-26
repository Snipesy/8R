#!/usr/bin/env python3
"""Compare jadx failures between an input app and 8R's output, per input class and error kind.

Usage: jadx-diff.py JADX_OUT_DIR_OF_INPUT JADX_OUT_DIR_OF_OUTPUT 8r-mapping.txt
(run `jadx -d DIR` on each first). "new" failures in classes 8R created (split subclasses) usually
moved from the merged base class, where they count as "gone"."""
import re,os,sys,collections
IN, OUT, mapping = sys.argv[1], sys.argv[2], sys.argv[3]
cls={}
for line in open(mapping):
    if not line.startswith(' '):
        m=re.match(r'(\S+) -> (\S+):',line)
        if m: cls[m.group(1)]=m.group(2)
def kind(block):
    l=block.split('\n')
    exc=l[1].strip() if len(l)>1 else ''
    exc=re.sub(r'r\d+v\d+','rX',exc); exc=re.sub(r'B:\d+:0x[0-9a-f]+','B',exc); exc=re.sub(r'0x[0-9a-f]+','0x',exc)
    exc=re.sub(r'(new type|Code variable not set in rX) .*',r'\1 …',exc)
    return l[0].strip()+' | '+exc[:110]
def fails(d, is_out):
    out=collections.Counter()
    for root,_,fs in os.walk(d):
        for f in fs:
            if not f.endswith('.java'): continue
            p=os.path.join(root,f); txt=open(p,errors='replace').read()
            rel=os.path.relpath(p,d+'/sources')[:-5].replace('/','.')
            c=rel[len('defpackage.'):] if rel.startswith('defpackage.') else rel
            if is_out: c=cls.get(c,c)
            for m in re.finditer(r'/\*  JADX ERROR: [^\n]*\n[^\n]*',txt):
                out[(c,kind(m.group(0)))]+=1
            for m in re.finditer(r'JADX WARN: Code restructure failed',txt):
                out[(c,'restructure')]+=1
    return out
a=fails(IN, False); b=fails(OUT, True)
new=collections.Counter(); gone=collections.Counter(); newk=collections.Counter()
for k in set(a)|set(b):
    d=b[k]-a[k]
    if d>0: new[k]=d; newk[k[1]]+=d
    elif d<0: gone[k]=-d
print('total in',sum(a.values()),'out',sum(b.values()),'new',sum(new.values()),'gone',sum(gone.values()))
print('NEW by kind:')
for k,n in newk.most_common(25): print(n,k)
print('examples:')
for k,n in sorted(new.items())[:40]: print(n,k[0],'::',k[1][:90])
