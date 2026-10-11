d=$(mktemp -d); cd "$d"
echo one > f; echo two >> f; cat f
cat < f | wc -l
echo err 1>&2 2>/dev/null
{ echo out; echo err >&2; } 2>&1 | sort
echo to-null > /dev/null; echo "status $?"
cat <<EOF2
heredoc $HOME_LIKE expanded $((1+1))
  indented stays
EOF2
cat <<'EOF3'
not $expanded
EOF3
cat <<-EOF4
	tab stripped
	EOF4
x=value; cat <<EOF5 | tr a-z A-Z
piped $x
EOF5
exec 3> fd3; echo via-fd3 >&3; exec 3>&-; cat fd3
while read -r line; do echo "got: $line"; done < f
for i in 1 2; do echo $i; done > loop.out; cat loop.out
if true; then echo in-if; fi > if.out; cat if.out
echo a b c | { read x y; echo "$y-$x"; }
printf 'l1\nl2\nl3\n' | while read l; do echo "[$l]"; done
cat nonexistent 2>/dev/null || echo "missing file"
echo hi > out1 > out2; cat out1 out2
set -C; echo x > f 2>/dev/null || echo noclobber; echo y >| f; cat f; set +C
cd /; rm -r "$d"
