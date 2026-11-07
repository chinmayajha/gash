wc() { command wc "$@" | tr -d ' '; }
# here-documents and here-strings
name=World
cat <<EOF
c1: Hello $name
c1: cmd $(echo sub) arith $((1+2))
c1: escaped \$name and \`tick\` and backslash \\
c1: 'single quotes' "double quotes" kept
EOF
cat <<'EOF'
c2: no $name expansion $(echo x) \$ \\
EOF
cat <<"EOF"
c3: dq delimiter $name
EOF
cat <<E\OF
c4: escaped delimiter $name
EOF
cat <<-EOF
	c5: tab stripped $name
		c5: two tabs stripped
	EOF
cat <<-'EOF'
	c6: quoted tab-strip $name
	EOF
cat <<EOF | sed 's/^/piped /'
c7: into pipe
EOF
while read -r a b; do echo "c8: $a-$b"; done <<EOF
x 1
y 2
EOF
f() { cat <<EOF
c9: in function arg=$1
EOF
}
f arg1
x=$(cat <<EOF
c10: in cmdsub
EOF
)
echo "$x"
cat <<EOF
c11: line continuation \
joined
EOF
cat <<'EOF'
c12: no continuation in quoted \
next
EOF
cat <<EOF
c13: ${name:-default} ${#name} ${name/World/There}
EOF
cat << EOF
c14: space before delimiter
EOF
cat <<EOF
EOF
echo "c15: empty heredoc ok"
cat <<EOF1; cat <<EOF2
c16: first
EOF1
c16: second
EOF2
cat <<EOF
c17: "$name" '$name'
  c17: leading spaces kept
EOF
read -r v <<EOF
c18: read from heredoc
EOF
echo "$v"
cat <<EOF >/dev/null
discarded
EOF
echo "c19: redirect after heredoc"
cat <<'END' | wc -l | sed 's/^/c20: /'
1
2
3
END
arr=(a b)
cat <<EOF
c21: ${arr[1]} ${arr[*]}
EOF
cat <<EOF
c22: EOF not at start EOF
 EOF
EOF
cat <<-EOF
  c23: spaces are not stripped by <<-
	  c23: only leading tabs
EOF
cat <<<"c24: here-string"
cat <<< 'c25: $name not expanded'
cat <<< "c26: $name expanded"
cat <<< $'c27: ansi\tstring'
wc -l <<< "one line" | sed 's/^/c28: /'
x="multi
line"
wc -l <<< "$x" | sed 's/^/c29: /'
read -r first second <<< "c30: words here"
echo "$first $second"
tr a-z A-Z <<< "c31: upper"
cat <<< ""  | wc -c | sed 's/^/c32: bytes /'
cat <<EOF
c33: $(cat <<INNER
nested heredoc
INNER
)
EOF
cat <<"END_OF_TEXT"
c34: long delimiter
END_OF_TEXT
if true; then
  cat <<EOF
c35: inside if block
EOF
fi
cat <<EOF
c36: tab	in body kept
EOF
cat <<EOF
c37: arithmetic $(( 6 * 7 )) and \$(( literal ))
EOF
