# echo builtin: flags and escapes
echo "c1:" hello world
echo -n "c2: no newline"; echo "|"
echo -e "c3: tab\there"
echo -E "c4: raw\tnot-tab"
echo "c5: default no escapes\n"
echo -e "c6: a\\\\b"
echo -e "c7: octal \0101\0102 hex \x41\x62"
echo -e "c8: before\c after"; echo "|c8 end"
echo -ne "c9: combined\n"
echo -en "c10: combined2\n"
echo -- "c11: dashdash"
echo -x "c12: unknown flag printed"
echo -n
echo "c13: after empty -n"
echo
echo "c14: after bare echo"
echo -e 'c15: \e|\a|\v|\f' | cat -v
echo "c16:" "multiple   spaces" preserved
echo c17: unquoted    collapsed     spaces
echo -e "c18: \x7e\x7E"
echo -e 'c19: Ab'
echo -e "c20: trailing backslash \\"
echo -nE "c21: -nE"; echo
echo -e "c22: \0 nul" | cat -v
echo -e "c23: \101 not octal without zero"
echo -e "c24: \\n literal? \n real"
echo -ee "c25: double flag\t"
echo -n -e "c26: separate\tflags"; echo
echo "-n" "c27: quoted flag still a flag"
echo "c28:" -n
echo -e "c29: \x4g partial hex" | cat -v
echo -e "c30: \q unknown"
shopt -s xpg_echo
echo "c31: xpg \t on"
shopt -u xpg_echo
echo "c32: xpg \t off"
echo c33: a\ b\ \ c
echo "c34: $(echo -n inner)|"
echo c35: '' empty-arg ''|cat -vet
