sleep 0.2 & pid=$!
[ -n "$pid" ] && echo has-pid
wait $pid; echo "wait status $?"
(exit 5) & wait $!; echo "bg status $?"
for i in 1 2 3; do (sleep 0.$((i * 2)); echo bg$i) & done; wait
echo all-done
x=1; { x=2; } & wait; echo "x=$x"
