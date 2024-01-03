#!/bin/bash

VERSION=$1
TARGET=$2 

mkdir -p dist/apicli-${VERSION}-${TARGET}

mv target/${TARGET}/release/apicli dist/apicli-${VERSION}-${TARGET}/
mv target/${TARGET}/release/apid dist/apicli-${VERSION}-${TARGET}/

cp -R scripts dist/apicli-${VERSION}-${TARGET}/
cp -R templates dist/apicli-${VERSION}-${TARGET}/
cp -R etc dist/apicli-${VERSION}-${TARGET}/

cd dist
tar -czvf apicli-${VERSION}-${TARGET}.tar.gz apicli-${VERSION}-${TARGET}/
cd ..

