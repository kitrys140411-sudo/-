echo 'single $HOME "q"'
echo "double 'q' \$ \" \\ \` x"
echo a\ b\	c
echo "a""b"'c'd
echo ""
echo "multi
line"
x='it'"'"'s'
echo "$x"
printf '%s\n' '\n stays'
echo "tab	inside"
printf '%s|' "a b" c "" d; echo
