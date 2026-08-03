-- R43 — os enfeites do perfil: rosto, capa e moldura.
--
-- Três colunas de texto, e não três tabelas: o que se guarda é a **escolha**,
-- e o catálogo é código (`enfeites.rs`), pela mesma razão que a lista de
-- conquistas é código (§48) — a regra "este rosto abre com aquela conquista"
-- é um vínculo entre duas listas do programa, não um dado que alguém edita.
--
-- Sem `CHECK` de valores: a lista válida muda com o código, e um CHECK aqui
-- exigiria migração toda vez que um rosto novo entrasse. Quem valida é o
-- handler, que também é quem sabe conferir o desbloqueio.
--
-- `NULL` é "não escolheu", e é o estado de todo mundo hoje: a marca derivada do
-- nome (R42) continua sendo o padrão de quem não escolheu rosto.
ALTER TABLE perfil
    ADD COLUMN avatar   text,
    ADD COLUMN capa     text,
    ADD COLUMN moldura  text;
