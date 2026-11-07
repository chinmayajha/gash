# aliases (expand_aliases) and special parameters
shopt -s expand_aliases
alias greet='echo c1: hello'
greet
alias say='echo'
say "c2: alias with args"
alias e2='echo c3:'
e2 chained args
alias with_space='echo '
alias target='c4: target expanded'
with_space target
unalias greet
type -t greet >/dev/null || echo "c5: unaliased"
alias ll='echo c6: ll'; ll 2>/dev/null || echo "c6: same-line alias not yet active"
ll
\ll 2>/dev/null || echo "c7: escaped alias skipped"
"ll" 2>/dev/null || echo "c8: quoted alias skipped"
alias | grep -c '^alias ' | sed 's/^/c9: count /'
alias say | sed 's/^/c10: /'
f() { e2 inside function defined after alias; }
alias e2='echo c11:'
f
e2 new definition
echo "c12: $#"
set -- a b c
echo "c13: $#"
true; echo "c14: $?"
false; echo "c15: $?"
(exit 42); echo "c16: $?"; echo "c17: $?"
echo a b last >/dev/null; echo "c18: $_"
[[ $$ == $BASHPID ]] && echo "c19: \$\$ equals BASHPID in main"
( [[ $$ != $BASHPID ]] && echo "c20: differ in subshell" )
echo "c21: $(( RANDOM >= 0 && RANDOM <= 32767 ))"
r1=$RANDOM; r2=$RANDOM; [[ $r1 =~ ^[0-9]+$ && $r2 =~ ^[0-9]+$ ]] && echo "c22: numeric"
echo "c23: line $LINENO"
f2() { echo "c24: line in func $LINENO"; }
f2
x=1; echo "c25: assignment status $?"
x=$(exit 3); echo "c26: cmdsub assignment status $?"
unalias -a; alias | command wc -l | tr -d ' ' | sed 's/^/c27: after unalias -a: /'
alias multi='echo c28: one; echo c28: two'
multi
alias nested_alias='say c29: via nested alias'
alias say='echo'
nested_alias
alias q="echo 'c30: quoted alias'"
q
if true; then
  alias inblock='echo c31: alias in block'
fi
inblock 2>/dev/null || echo "c31: alias defined in block works after"
echo "c32: ${#}"
set --
echo "c33: [$*] [$@] $#"
echo "c34: $(( $? + 0 ))"
