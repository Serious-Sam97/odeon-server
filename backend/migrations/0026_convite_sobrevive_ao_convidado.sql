-- Conserta um defeito que a 0025 introduziu e que só a limpeza encontrou.
--
-- ## O sintoma
--
-- Apagar um convidado que já tinha usado o convite dava erro de banco:
--
-- ```text
-- ERROR: new row for relation "convite" violates check constraint
--        "convite_uso_completo"
-- CONTEXT: UPDATE ONLY "convite" SET "usado_por" = NULL
-- ```
--
-- ## A causa: duas regras minhas brigando
--
-- A 0025 escreveu as duas, e cada uma sozinha estava certa:
--
--  * `usado_por uuid REFERENCES app_user(id) ON DELETE SET NULL` — apagar uma
--    pessoa não deve apagar o registro de que um convite foi usado;
--  * `CHECK ((usado_em IS NULL AND usado_por IS NULL) OR (ambos preenchidos))`
--    — "usado sem saber por quem" não deveria ser alcançável.
--
-- Juntas, elas se contradizem exatamente no caso que a primeira existe pra
-- permitir. O `SET NULL` produz `usado_em` preenchido com `usado_por` nulo, e
-- o CHECK recusa. O resultado é uma linha de `app_user` **indelével**, e o
-- administrador descobre isso por um erro de constraint no meio da tela.
--
-- ## O conserto: o CHECK é que estava errado
--
-- "Usado por alguém que não está mais aqui" **é** um estado legítimo — é o que
-- resta de um convidado removido, e é informação honesta. O que continua
-- proibido é o contrário: `usado_por` sem `usado_em`, que seria dizer quem
-- usou sem dizer que foi usado.
--
-- A lição vale mais que o conserto: uma constraint que descreve o estado
-- "normal" pode proibir o estado "residual" que outra regra produz de
-- propósito. As duas foram escritas com dez linhas de distância no mesmo
-- arquivo, e mesmo assim.

ALTER TABLE convite DROP CONSTRAINT IF EXISTS convite_uso_completo;

ALTER TABLE convite ADD CONSTRAINT convite_uso_coerente
    -- `usado_por` sem `usado_em` continua proibido: dizer quem usou sem dizer
    -- que foi usado é a metade que nunca faz sentido.
    CHECK (usado_por IS NULL OR usado_em IS NOT NULL);

COMMENT ON COLUMN convite.usado_por IS
    'Quem resgatou. NULL com usado_em preenchido = a pessoa foi removida depois.';
