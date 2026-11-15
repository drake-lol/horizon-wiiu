#!/bin/sh
# usage: asm.sh <shader-dir> <name>  -> <shader-dir>/<name>.gsh from <name>.vsh + <name>.psh (needs the latte-cfg image)
d=$1; n=$2
docker run --rm -v "${OUT:-$PWD/latte-out}":/out -v "$d":/sh latte-cfg sh -c "cd /sh && /out/latte-assembler assemble --vsh=$n.vsh --psh=$n.psh $n.gsh"
