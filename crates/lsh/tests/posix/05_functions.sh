greet() { echo "hello $1"; }
greet world
fact() {
  if [ "$1" -le 1 ]; then echo 1; return; fi
  echo $(( $1 * $(fact $(( $1 - 1 ))) ))
}
fact 6
ret() { return 3; }; ret; echo "status $?"
args() { echo "$#:$*"; shift; echo "$#:$*"; }; args a b c
echo "after: $#"
counter=0; inc() { counter=$((counter+1)); }; inc; inc; echo counter=$counter
scope() { local v=local; echo "in $v"; }; v=global; scope; echo "out $v"
nested() { inner() { echo inner-called; }; inner; }; nested; inner
f() { echo "f with $1"; }; f() { echo "redefined $1"; }; f x
unset -f f; f 2>/dev/null || echo "f gone"
recurse() { [ $1 -eq 0 ] && return 0; echo -n "$1 "; recurse $(( $1 - 1 )); }; recurse 5; echo
