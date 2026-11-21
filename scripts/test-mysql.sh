#!/bin/sh
# Sobe um MySQL descartável em Docker, cria o usuário de leitura e roda os testes
# de integração. Senhas são aleatórias a cada execução e não saem deste processo.
set -eu

image="${SELECTALL_MYSQL_IMAGE:-mysql:8.4}"
name="selectall-test-mysql"
port="${SELECTALL_MYSQL_PORT:-33062}"
root_password=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
read_password=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')

docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" -e MYSQL_ROOT_PASSWORD="$root_password" -e MYSQL_DATABASE=pdv \
  -p "127.0.0.1:$port:3306" "$image" >/dev/null
trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT

# O servidor reinicia uma vez durante a inicialização da imagem oficial.
ready=0
for _ in $(seq 1 90); do
  if docker exec "$name" mysql -uroot -p"$root_password" -h127.0.0.1 -e 'SELECT 1' >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 2
done
[ "$ready" = 1 ] || { echo "MySQL não ficou pronto" >&2; exit 1; }

docker exec -i "$name" mysql --default-character-set=utf8mb4 -uroot -p"$root_password" pdv <<SQL
CREATE USER 'leitura'@'%' IDENTIFIED BY '$read_password';
GRANT SELECT ON pdv.* TO 'leitura'@'%';
CREATE TABLE parametros (
  cod_loja INT NOT NULL,
  versao_pdv VARCHAR(16) NOT NULL,
  qtd_max_itens INT NOT NULL,
  preco DECIMAL(10,2),
  data_carga DATE,
  ultima_carga DATETIME(6),
  observacao VARCHAR(40)
);
INSERT INTO parametros VALUES
  (101, '7.12.4', 999, 12.50, '2026-11-21', '2026-11-21 08:14:03.000120', NULL),
  (101, '7.12.3', 500, 3.99, '2026-11-20', '2026-11-20 23:59:59', 'ação');
-- Função que grava: só a transação READ ONLY do servidor impede o SELECT de alterar dados.
SET GLOBAL log_bin_trust_function_creators = 1;
CREATE TABLE marcas (id INT AUTO_INCREMENT PRIMARY KEY);
DELIMITER //
CREATE FUNCTION marca() RETURNS INT MODIFIES SQL DATA SQL SECURITY DEFINER
BEGIN
  INSERT INTO marcas VALUES (NULL);
  RETURN 1;
END //
DELIMITER ;
GRANT EXECUTE ON FUNCTION pdv.marca TO 'leitura'@'%';
SQL

SELECTALL_TEST_MYSQL_HOST=127.0.0.1 \
SELECTALL_TEST_MYSQL_PORT="$port" \
SELECTALL_TEST_MYSQL_USER=leitura \
SELECTALL_TEST_MYSQL_PASSWORD="$read_password" \
  cargo test -p selectall-core --test mysql_real -- --ignored --test-threads=1
