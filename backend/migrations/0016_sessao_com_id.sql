-- Cada sessão ganha um identificador próprio.
--
-- A chave primária de `auth_session` é o `token_hash`, e isso bastava enquanto
-- a única operação era "revogar todas". Para encerrar UM aparelho pela tela de
-- administração é preciso nomear a sessão — e o `token_hash` não serve para
-- isso: é material derivado do segredo de autenticação, e valor dessa natureza
-- não deve trafegar até o navegador só para virar o `key` de uma linha de
-- tabela.
--
-- O `id` é gerado para as linhas existentes pelo próprio DEFAULT, então
-- ninguém é deslogado por esta migração.
ALTER TABLE auth_session
    ADD COLUMN IF NOT EXISTS id uuid NOT NULL DEFAULT gen_random_uuid();

CREATE UNIQUE INDEX IF NOT EXISTS auth_session_id_idx ON auth_session (id);
