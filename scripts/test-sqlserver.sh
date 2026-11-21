#!/bin/sh
# Sobe um SQL Server descartável em Docker, cria os usuários de leitura e de escrita e
# roda os testes de integração. Senhas são aleatórias a cada execução e não saem deste processo.
set -eu

image="${SELECTALL_SQLSERVER_IMAGE:-mcr.microsoft.com/mssql/server:2022-latest}"
name="selectall-test-sqlserver"
port="${SELECTALL_SQLSERVER_PORT:-14332}"
# O `sa` exige três tipos de caractere: o hexadecimal dá minúsculas e dígitos.
sa_password="$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')A"
read_password=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
write_password=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
sqlcmd="/opt/mssql-tools18/bin/sqlcmd"

docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" -e ACCEPT_EULA=Y -e MSSQL_PID=Developer -e MSSQL_SA_PASSWORD="$sa_password" \
  -p "127.0.0.1:$port:1433" "$image" >/dev/null
trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT

ready=0
for _ in $(seq 1 90); do
  if docker exec "$name" "$sqlcmd" -C -S 127.0.0.1 -U sa -P "$sa_password" -Q 'SELECT 1' >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 2
done
[ "$ready" = 1 ] || { echo "SQL Server não ficou pronto" >&2; exit 1; }

docker exec -i "$name" sh -c 'cat > /tmp/selectall.sql' <<SQL
CREATE LOGIN leitura WITH PASSWORD = '$read_password', CHECK_POLICY = OFF;
-- KILL exige ALTER ANY CONNECTION: só o usuário de escrita tem, para cobrir os dois caminhos do cancelamento.
CREATE LOGIN escrita WITH PASSWORD = '$write_password', CHECK_POLICY = OFF;
GRANT ALTER ANY CONNECTION TO escrita;
CREATE DATABASE pdv;
CREATE DATABASE loja1;
CREATE DATABASE loja2;
GO
USE pdv;
CREATE USER leitura FOR LOGIN leitura;
ALTER ROLE db_datareader ADD MEMBER leitura;
CREATE TABLE parametros (
  cod_loja INT NOT NULL,
  versao_pdv VARCHAR(16) NOT NULL,
  qtd_max_itens INT NOT NULL,
  preco DECIMAL(10,2),
  data_carga DATE,
  ultima_carga DATETIME2(6),
  observacao NVARCHAR(40),
  ativo BIT
);
INSERT INTO parametros VALUES
  (101, '7.12.4', 999, 12.50, '2026-11-21', '2026-11-21 08:14:03.000120', NULL, 1),
  (101, '7.12.3', 500, 3.99, '2026-11-20', '2026-11-20 23:59:59', N'ação', 0);
GO
USE loja1;
CREATE USER escrita FOR LOGIN escrita;
CREATE TABLE produto (cod INT PRIMARY KEY, bloqueado TINYINT NOT NULL);
INSERT INTO produto VALUES (1, 0), (2, 0), (3, 0), (4, 0), (5, 0);
GRANT SELECT, INSERT, UPDATE, DELETE ON produto TO escrita;
GO
USE loja2;
CREATE USER escrita FOR LOGIN escrita;
CREATE TABLE produto (cod INT PRIMARY KEY, bloqueado TINYINT NOT NULL);
INSERT INTO produto VALUES (1, 0), (2, 0), (3, 0), (4, 0), (5, 0);
GRANT SELECT, INSERT, UPDATE, DELETE ON produto TO escrita;
GO
SQL
docker exec "$name" "$sqlcmd" -C -b -f 65001 -S 127.0.0.1 -U sa -P "$sa_password" -i /tmp/selectall.sql >/dev/null

SELECTALL_TEST_SQLSERVER_HOST=127.0.0.1 \
SELECTALL_TEST_SQLSERVER_PORT="$port" \
SELECTALL_TEST_SQLSERVER_USER=leitura \
SELECTALL_TEST_SQLSERVER_PASSWORD="$read_password" \
SELECTALL_TEST_SQLSERVER_WRITE_USER=escrita \
SELECTALL_TEST_SQLSERVER_WRITE_PASSWORD="$write_password" \
  cargo test -p selectall-core --test sqlserver_real -- --ignored --test-threads=1
