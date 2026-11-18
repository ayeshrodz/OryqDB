#!/usr/bin/env python3
"""Check documentation links and exercise the proposed key/bounds/ABI contracts.

Run: python3 tools/validate_design_docs.py
Uses Python's standard library plus local C and C++ compilers.
These design-level checks do not test an implemented database.
"""
from pathlib import Path
from fractions import Fraction
from decimal import Decimal
import random, struct, re, subprocess, tempfile
root=Path(__file__).resolve().parents[1]
# Prototype exact proposed numeric transform; oracle compares independent Fraction values.
def key(v):
 v=Fraction(v)
 if not v:return b'\x20\x20'
 sign=v<0; v=abs(v); n,d=v.numerator,v.denominator; a=b=0
 while d%2==0:d//=2;a+=1
 while d%5==0:d//=5;b+=1
 assert d==1
 scale=max(a,b); coeff=n*2**(scale-a)*5**(scale-b)
 digits=str(coeff); exponent=len(digits)-scale-1;digits=digits.rstrip('0')
 mag=(exponent+2**31).to_bytes(4,'big')+bytes(16+int(c) for c in digits)+b'\0'
 return b'\x20'+(b'\x10'+bytes(x^255 for x in mag) if sign else b'\x30'+mag)
rng=random.Random(31827)
vals=[Fraction(0),Fraction(1,10),Fraction(-1,10),Fraction(10),Fraction(1,2)]
for x in [2**53,2**62,2**63-1,-2**63]:vals.extend(Fraction(x+i) for i in range(-1100,1101))
for _ in range(6000):
 bits=rng.getrandbits(64); f=struct.unpack('>d',bits.to_bytes(8,'big'))[0]
 if abs(f)!=float('inf') and f==f:vals.append(Fraction(f))
 vals.append(Fraction(rng.randint(-2**63,2**63-1)))
 vals.append(Fraction(Decimal(rng.randint(-10**34,10**34)).scaleb(rng.randint(-100,100))))
vals += [Fraction(float.fromhex('0x0.0000000000001p-1022')),Fraction(float.fromhex('0x1.fffffffffffffp+1023'))]
expected=sorted(set(vals));actual=sorted(set(vals),key=key)
assert actual==expected
assert key(10)==key(10.0)==key(Decimal('10.0'))
assert key(Decimal('0.1'))!=key(0.1)
assert len({key(2**62+i) for i in range(1024)})==1024
print(f'Numeric prototype: exact ordering/equality passed for {len(expected)} unique rational inputs; 1024 adjacent large integers distinct.')
def escaped(s):return s.encode().replace(b'\0',b'\0\xff')
def succ(p):
 a=bytearray(p)
 for i in range(len(a)-1,-1,-1):
  if a[i]!=255:a[i]+=1;return bytes(a[:i+1])
 return None
strings=['','a','a\0','a\U0010ffff','a\U0010ffffsuffix','ab','b','\U0010ffff','\0','\0tail']
for prefix in strings:
 lo=b'\x40'+escaped(prefix);hi=succ(lo)
 for s in strings:
  k=b'\x40'+escaped(s)+b'\0\0'
  assert (lo<=k and (hi is None or k<hi)) == s.startswith(prefix),(prefix,s)
print('Prefix-bound prototype: 100 Unicode/NUL/empty/maximal-scalar cases passed.')
# C header is an actual syntactic contract even before implementation exists.
ffi=(root/'docs/ffi.md').read_text();header=re.search(r'```c\n(.*?)```',ffi,re.S).group(1)
with tempfile.TemporaryDirectory(prefix='oryq-design-check-') as tmp:
 header_path=Path(tmp)/'oryq_review.h'
 header_path.write_text(header)
 for lang,compiler in [('c','cc'),('c++','c++')]:
  subprocess.run([compiler,'-x',lang,'-fsyntax-only','-Wall','-Wextra','-Werror',str(header_path)],check=True)
print('Illustrative ABI header: C and C++ syntax checks passed with warnings treated as errors.')
# Local Markdown targets and heading fragments (ignore intentionally uncreated code paths).
def slug(s):
 s=re.sub(r'\[([^]]+)\]\([^)]+\)',r'\1',s).lower()
 s=re.sub(r'[^\w\- ]','',s).replace(' ','-');return s
missing=[];fragments=[];fences=[]
for p in root.rglob('*.md'):
 s=p.read_text()
 if len(re.findall(r'^```',s,re.M))%2:fences.append(str(p.relative_to(root)))
 for target in re.findall(r'(?<!!)\[[^]\n]+\]\(([^)\s]+)\)',s):
  if re.match(r'\w+://',target) or target.startswith('mailto:'):continue
  name,_,frag=target.partition('#');q=(p.parent/name).resolve() if name else p
  if not q.exists():missing.append((str(p.relative_to(root)),target));continue
  if frag and q.suffix=='.md':
   headings=[slug(t) for t in re.findall(r'^#{1,6}\s+(.+)',q.read_text(),re.M)]
   if frag not in headings:fragments.append((str(p.relative_to(root)),target))
print('Missing local link targets:',missing)
print('Unresolved heading fragments:',fragments)
print('Unbalanced fences:',fences)
assert not missing and not fragments and not fences
