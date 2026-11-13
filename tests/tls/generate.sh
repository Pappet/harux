#!/bin/sh
# Static certificates for the TLS tests in tests/send_tcp.rs (#273).
# Committed as files, so no crate generates them at test time. Re-run this
# script from tests/tls/ only to replace them (they are valid until 2126).
#
#   ca.pem                       test CA ("Harux Test CA"; its key is not kept)
#   server.pem / server.key      localhost + 127.0.0.1, issued by the test CA
#   wrongname.pem / wrongname.key  elsewhere.example only, issued by the test CA
#   client.pem / client.key      Harux's client certificate, issued by the test CA
#   other-ca.pem                 a second CA nobody trusts ("Unknown Test CA")
#   untrusted.pem / untrusted.key  localhost, issued by the unknown CA
#   other.key                    a key that belongs to no certificate here
set -eu
DAYS=36500
key() { openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$1" 2>/dev/null; }
ca() { # name file subject
    key "$1.key"
    openssl req -x509 -new -key "$1.key" -sha256 -days $DAYS -subj "$2" -out "$1.pem" \
        -addext "basicConstraints=critical,CA:TRUE" \
        -addext "keyUsage=critical,keyCertSign,cRLSign"
}
leaf() { # name subject san eku issuer
    key "$1.key"
    openssl req -new -key "$1.key" -subj "$2" -out "$1.csr"
    printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=%s\nsubjectAltName=%s\n' "$4" "$3" > "$1.ext"
    openssl x509 -req -in "$1.csr" -CA "$5.pem" -CAkey "$5.key" -CAcreateserial -sha256 \
        -days $DAYS -extfile "$1.ext" -out "$1.pem" 2>/dev/null
    rm -f "$1.csr" "$1.ext"
}
ca ca "/O=Harux Tests/CN=Harux Test CA"
ca other-ca "/O=Elsewhere/CN=Unknown Test CA"
leaf server "/CN=localhost" "DNS:localhost,IP:127.0.0.1" serverAuth ca
leaf wrongname "/CN=elsewhere.example" "DNS:elsewhere.example" serverAuth ca
leaf client "/CN=harux-client" "DNS:harux-client" clientAuth ca
leaf untrusted "/CN=localhost" "DNS:localhost,IP:127.0.0.1" serverAuth other-ca
key other.key
rm -f ca.key other-ca.key ./*.srl
