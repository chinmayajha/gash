# pipelines carrying data: pipes into blocks/loops/subshells, |&, multi-line pipelines
echo "abc" | tr a-z A-Z | sed 's/^/c1: /'
printf 'b\na\nc\n' | sort | head -n 2 | tr '\n' ' ' | sed 's/^/c2: /'; echo
{ echo err-line >&2; } 2>&1 | sed 's/^/c3: /'
{ echo stderr-via-pipe-amp >&2; } |& sed 's/^/c4: /'
f() { echo "c5: from func"; return 3; }
f | cat; echo "c5: status ${PIPESTATUS[0]} ${PIPESTATUS[1]}"
echo one \
  | sed 's/one/two/' \
  | sed 's/^/c6: /'
echo x | (read v; echo "c7: subshell got $v")
out=$(echo a; echo b | tr b B); echo "c8: $(echo $out)"
r=$(true | false | true; echo "${PIPESTATUS[1]}"); echo "c9: $r"
cnt=$(printf '%s\n' a b c d | wc -l); echo "c10: $cnt"
echo hi | cat - | cat | sed 's/^/c11: /'
echo "c12: $(echo piped-into-cmdsub | cat)"
printf 'x\ny\n' |
  while read -r l; do
    echo "c13: $l"
  done
true | false
echo "c14: ${PIPESTATUS[*]} then ${PIPESTATUS[*]}"
! { false; }; echo "c15: $?"
v=$(! true); echo "c16: $?"
set -o pipefail; ! false | true; echo "c17: negated pipefail $?"; set +o pipefail
seq_out=$(for i in 1 2 3; do echo $i; done | sort -r | tr '\n' ' '); echo "c18: $seq_out"
echo "a b c" | { read -r first rest; echo "c19: [$first] [$rest]"; }
{ echo 1; echo 2; } | { while read -r n; do echo "c20: got $n"; done; }
x=1; { x=2; } | cat; echo "c21: x after group in pipe $x"
(exit 0) | (exit 0) | (exit 9); echo "c22: ${#PIPESTATUS[@]} ${PIPESTATUS[2]}"
