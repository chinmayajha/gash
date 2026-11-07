# basic redirections: > >> < 2>&1 &> 2>/dev/null >&2
tmp=$(mktemp -d) || exit 1
cd "$tmp" || exit 1
echo first > f
echo second >> f
cat f | sed 's/^/c1: /'
echo overwrite > f
echo "c2: $(cat f)"
wc -l < f | sed 's/^/c3: /'
read -r line < f; echo "c4: $line"
{ echo out; echo err >&2; } > o.txt 2> e.txt
echo "c5: out=[$(cat o.txt)] err=[$(cat e.txt)]"
{ echo out; echo err >&2; } > both.txt 2>&1
echo "c6: $(sort both.txt | tr '\n' ' ')"
{ echo out; echo err >&2; } &> amp.txt
echo "c7: $(sort amp.txt | tr '\n' ' ')"
{ echo more; } &>> amp.txt
echo "c8: $(wc -l < amp.txt)"
x=$( { echo out; echo err >&2; } 2>&1 >/dev/null )
echo "c9: order matters [$x]"
x=$( { echo out; echo err >&2; } >/dev/null 2>&1 )
echo "c10: both null [$x]"
x=$( { echo e1 >&2; } 2>&1 ); echo "c11: [$x]"
y=$( cat nonexistent 2>/dev/null ); echo "c12: [$y] $?"
cat nonexistent 2>/dev/null; echo "c13: status $?"
echo data > in.txt
cat < in.txt > out.txt; echo "c14: $(cat out.txt)"
< in.txt cat | sed 's/^/c15: /'
> pre.txt echo prefix-redirect; echo "c16: $(cat pre.txt)"
echo a 1>one.txt; echo "c17: $(cat one.txt)"
: > trunc.txt; echo "c18: size $(wc -c < trunc.txt)"
echo hello > "file with space"; echo "c19: $(cat "file with space")"
name=dyn; echo dynamic > "$name.txt"; echo "c20: $(cat dyn.txt)"
echo multi > m1 > m2; echo "c21: [$(cat m1)] [$(cat m2)]"
echo x 2>/dev/null >/dev/null; echo "c22: ok"
printf 'a\nb\n' >> app.txt; printf 'c\n' >> app.txt; echo "c23: $(wc -l < app.txt)"
echo "c24: $(< in.txt)"
x=$( { echo "to stderr" >&2 2>/dev/null; } 2>&1 ); echo "c25a: [$x]"
echo "c25: after stderr echo"
x=$(echo "via fd2" 2>&1 1>&2); echo "c26: [$x]"
( echo "c27: subshell out"; echo "c27: subshell err" >&2 ) 2>&1 | sort
cat < /dev/null; echo "c28: devnull input status $?"
x=$(cat < /dev/null); echo "c29: [$x]"
echo "line" > /dev/null; echo "c30: devnull output"
f=out2.txt; echo var-target > $f; echo "c31: $(cat out2.txt)"
( cd /nonexistent_dir_xyz ) 2>/dev/null; echo "c32: failed cd status $?"
( : < missing_input.txt ) 2>/dev/null; echo "c33: missing input status $?"
echo "x" > in2.txt; echo "y" >> in2.txt
while read -r v; do echo "c34: $v"; done < in2.txt
echo "c35: $(cat in2.txt | wc -l)"
cat in.txt in2.txt > cat.txt; echo "c36: $(tr '\n' ' ' < cat.txt)"
echo 2>err.txt "c37: redirect in middle"
echo "c38: [$(cat err.txt)]"
cd .. && rm -rf "$tmp"
