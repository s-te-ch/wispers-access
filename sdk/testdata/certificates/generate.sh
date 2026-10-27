#!/bin/sh
# Makes the certificates the SDK's certificate tests read. Valid for 100 years;
# the only key kept is the self-signed server's, for the test's TLS server.
set -e
cd "$(dirname "$0")"
days=36500
tmp=$(mktemp -d)
key() { openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$1" 2>/dev/null; }

# A CA, and a server certificate it issued.
key "$tmp/ca.key"
openssl req -x509 -new -key "$tmp/ca.key" -days $days -out ca.pem -subj "/CN=Example Root CA" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign"
key "$tmp/server.key"
openssl req -new -key "$tmp/server.key" -subj "/CN=app.internal.example" -out "$tmp/server.csr"
printf "basicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:app.internal.example\n" > "$tmp/server.ext"
openssl x509 -req -in "$tmp/server.csr" -CA ca.pem -CAkey "$tmp/ca.key" -CAcreateserial -CAserial "$tmp/ca.srl" \
  -days $days -extfile "$tmp/server.ext" -out server.pem 2>/dev/null

# A self-signed server certificate, and its key.
key self-signed.key
openssl req -x509 -new -key self-signed.key -days $days -out self-signed.pem -subj "/CN=app.internal" \
  -addext "basicConstraints=critical,CA:FALSE" -addext "extendedKeyUsage=serverAuth" \
  -addext "subjectAltName=DNS:app.internal,IP:10.1.2.3"

rm -r "$tmp"
