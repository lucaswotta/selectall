#!/bin/sh
# Sobe um PostgreSQL descartável em Docker, cria os usuários de leitura e de escrita e
# roda os testes de integração. Senhas são aleatórias a cada execução e não saem deste processo.
set -eu

image="${SELECTALL_POSTGRES_IMAGE:-postgres:17}"
name="selectall-test-postgres"
port="${SELECTALL_POSTGRES_PORT:-54322}"
admin_password=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
read_password=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
write_password=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')

docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" -e POSTGRES_PASSWORD="$admin_password" -e POSTGRES_DB=pdv \
  -p "127.0.0.1:$port:5432" "$image" >/dev/null
trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT

# Durante a inicialização o servidor temporário só escuta no soquete local; pela
# rede só responde o definitivo.
ready=0
for _ in $(seq 1 60); do
  if docker exec -e PGPASSWORD="$admin_password" "$name" \
    psql -h 127.0.0.1 -U postgres -d pdv -c 'SELECT 1' >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 1
done
[ "$ready" = 1 ] || { echo "PostgreSQL não ficou pronto" >&2; exit 1; }

docker exec -i -e PGPASSWORD="$admin_password" "$name" \
  psql -q -v ON_ERROR_STOP=1 -h 127.0.0.1 -U postgres -d pdv <<SQL
CREATE ROLE leitura LOGIN PASSWORD '$read_password';
CREATE TABLE parametros (
  cod_loja INT NOT NULL,
  versao_pdv VARCHAR(16) NOT NULL,
  qtd_max_itens INT NOT NULL,
  preco NUMERIC(10,2),
  data_carga DATE,
  ultima_carga TIMESTAMP(6),
  observacao VARCHAR(40),
  ativo BOOLEAN
);
INSERT INTO parametros VALUES
  (101, '7.12.4', 999, 12.50, '2026-11-21', '2026-11-21 08:14:03.000120', NULL, TRUE),
  (101, '7.12.3', 500, 3.99, '2026-11-20', '2026-11-20 23:59:59', 'ação', FALSE);
GRANT SELECT ON parametros TO leitura;
-- Função que grava: só a transação READ ONLY do servidor impede o SELECT de alterar dados.
CREATE TABLE marcas (id SERIAL PRIMARY KEY);
CREATE FUNCTION marca() RETURNS INT LANGUAGE sql SECURITY DEFINER
  AS 'INSERT INTO marcas DEFAULT VALUES RETURNING 1';
GRANT SELECT ON marcas TO leitura;

-- Correção: um banco por equipamento simulado, para conferir onde o piloto gravou.
CREATE ROLE escrita LOGIN PASSWORD '$write_password';
CREATE DATABASE loja1;
CREATE DATABASE loja2;
\c loja1
CREATE TABLE produto (cod INT PRIMARY KEY, bloqueado SMALLINT NOT NULL);
INSERT INTO produto VALUES (1, 0), (2, 0), (3, 0), (4, 0), (5, 0);
GRANT SELECT, INSERT, UPDATE, DELETE ON produto TO escrita;
\c loja2
CREATE TABLE produto (cod INT PRIMARY KEY, bloqueado SMALLINT NOT NULL);
INSERT INTO produto VALUES (1, 0), (2, 0), (3, 0), (4, 0), (5, 0);
GRANT SELECT, INSERT, UPDATE, DELETE ON produto TO escrita;
SQL

SELECTALL_TEST_POSTGRES_HOST=127.0.0.1 \
SELECTALL_TEST_POSTGRES_PORT="$port" \
SELECTALL_TEST_POSTGRES_USER=leitura \
SELECTALL_TEST_POSTGRES_PASSWORD="$read_password" \
SELECTALL_TEST_POSTGRES_WRITE_USER=escrita \
SELECTALL_TEST_POSTGRES_WRITE_PASSWORD="$write_password" \
  cargo test -p selectall-core --test postgres_real -- --ignored --test-threads=1
