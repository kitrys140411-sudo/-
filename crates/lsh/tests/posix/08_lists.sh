true && echo and1
false && echo no
false || echo or1
true || echo no
false && echo no || echo chain
! false && echo negated
! true; echo "neg status $?"
(x=sub; echo "in $x"); echo "out [$x]"
{ y=grp; }; echo "group $y"
(exit 3); echo "subshell exit $?"
true; echo $?; false; echo $?
echo a; echo b; echo c
false | true; echo "pipe status $?"
true | false; echo "pipe status $?"
echo x | tr x y | tr y z
cd /tmp && (cd / && pwd) && pwd
