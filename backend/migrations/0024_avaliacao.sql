-- R23 — a nota e a resenha.
--
-- ## Por que não é o `work_feedback` que já existe
--
-- O M5 criou `work_feedback` com `love | block | later`, e é tentador enfiar a
-- nota ali. São coisas diferentes, e o verbo denuncia:
--
-- | | o que é | pra quem fala |
-- |---|---|---|
-- | `work_feedback` | **instrução ao recomendador** — "nunca mais me ofereça" | pro sistema |
-- | `avaliacao` | **julgamento sobre a obra** — "isto é um 4" | pra você e pra casa |
--
-- Um `block` some com a obra do `/for-you` (a consulta do §8f já filtra por
-- ele); uma nota 2 não deve sumir com nada. E dá pra amar um filme que você
-- avaliaria 3 — as duas colunas não se substituem, e fundi-las forçaria uma a
-- mentir.
--
-- ## A nota é 1–5, e o texto é opcional
--
-- Meia-estrela e escala de 1–10 dão a impressão de precisão que ninguém tem
-- sobre um filme. Cinco degraus é o que uma locadora usava e é o que a pessoa
-- consegue distinguir.
--
-- `texto` é anulável de propósito: a maior parte das avaliações do mundo é só
-- a nota, e exigir prosa faria a nota não ser dada.
--
-- ## O que esta tabela NÃO pode fazer, e está no código
--
-- A regra do §4.6 do `IDEIAS.md`, que decidiu esta fase inteira: **sinal fraco
-- não manda no forte.** O M5 foi construído sobre "nada é declarado" e
-- "terminar > assistir", porque nota declarada é enviesada — as pessoas dão 5
-- estrelas pro que acham que deveriam gostar.
--
-- Então a nota entra na curadoria com peso **limitado**, e o limite é numérico,
-- não retórico: ver `affinity_of` em `curation/taste.rs`. Ela ajusta; nunca
-- inverte.

CREATE TABLE avaliacao (
    user_id      uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    work_id      uuid NOT NULL REFERENCES work(id) ON DELETE CASCADE,

    nota         int NOT NULL CHECK (nota BETWEEN 1 AND 5),
    texto        text,

    criado_em     timestamptz NOT NULL DEFAULT now(),
    atualizado_em timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (user_id, work_id)
);

-- "O que a casa achou desta obra" — a pergunta da ficha, e a que a R19 tornou
-- interessante: as notas de um círculo são as notas de gente que você conhece.
CREATE INDEX avaliacao_work_idx ON avaliacao (work_id);

-- "O que eu avaliei", pra retrospectiva da R24 e pro perfil inspecionável.
CREATE INDEX avaliacao_user_idx ON avaliacao (user_id, atualizado_em DESC);
