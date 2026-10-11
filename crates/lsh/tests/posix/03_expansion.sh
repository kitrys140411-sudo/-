v="a  b   c"
echo $v
echo "$v"
set -- $v; echo $#
IFS=,; w="x,,y,z"; set -- $w; echo $# "$2"; IFS=' 	
'
echo $((1+2*3)) $(( (1+2)*3 )) $((10/3)) $((10%3)) $((-5+2))
i=5; echo $((i+=2)) $i $((i-=1)) $i
echo $((0x10 + 010)) $(( 3 > 2 && 1 )) $(( 1 ? 7 : 8 ))
d=$(mktemp -d); cd "$d"
touch b.txt a.txt c.log .hidden
echo *.txt
echo *
echo .*hid*
echo "*.txt"
echo none*
mkdir sub; touch sub/x.txt sub/y.txt
echo */*.txt
echo [ab].txt
echo [!a]*.txt
set -f; echo *.txt; set +f
cd /; rm -r "$d"
HOME=/home/tester; echo ~ ~/docs
echo "~"
