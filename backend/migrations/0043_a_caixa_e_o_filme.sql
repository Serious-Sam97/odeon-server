-- A caixa da locadora passa a ser o **filme**, e não o rip.
--
-- ## O 403 que o app não conseguia prever
--
-- O cliente Android mediu, em 04/08/2026: «pegar a fita» em *007: A Serviço
-- Secreto de Sua Majestade* (1969) devolveu 403 enquanto a prateleira dizia
-- `possoPegar = 3` e nenhuma caixa emprestada. A mesma ação noutro filme
-- funcionou. A hipótese do app era que o filme não tem caixa.
--
-- Não é isso: **`alugar` aceita qualquer obra**; as 600 caixas da estante são
-- vitrine, não permissão. O que aquele filme tem de diferente é que ele existe
-- **duas vezes** no acervo:
--
--     a950f840-…  007: A Serviço Secreto de Sua Majestade (1969)
--     eddbfd12-…  007: A Serviço Secreto de Sua Majestade (1969)
--
-- Desde a R47 a biblioteca desenha os dois rips como **um cartão**. A locadora
-- continuou trancando por `work_id`, o id do rip. Daí as duas metades do
-- defeito, e elas são simétricas:
--
-- - o cliente não consegue prever a recusa, porque `emprestadas[].caixa_id`
--   traz o id do rip que está fora e o cartão conhece o do outro;
-- - o servidor não consegue impor a escassez, porque duas pessoas podem pegar
--   "o mesmo filme" ao mesmo tempo — cada uma num rip.
--
-- São 44 filmes com dois ou mais rips neste acervo. Em todos eles a fita é uma
-- promessa que a tabela não sustenta.
--
-- ## A chave
--
-- `caixa_chave` é a mesma identificação que agrupa versões na biblioteca
-- (R47) e que o guia passou a contar (R59): `external_ids->>'tmdb'` em filme,
-- o próprio id no resto. Uma regra só, nos três lugares.
--
-- Ela é **gravada na linha**, e não derivada na leitura, pelo mesmo motivo que
-- `exclusivo` é: o empréstimo nasce carregando o regime, e quem recusa
-- continua sendo o índice do banco (§35, §5). Se um rematch mudar o `tmdb` de
-- uma obra amanhã, as fitas já emprestadas não mudam de caixa no meio do
-- prazo.

ALTER TABLE emprestimo ADD COLUMN caixa_chave text;

UPDATE emprestimo e
SET caixa_chave = COALESCE(
    (SELECT CASE WHEN w.kind = 'movie' THEN w.external_ids->>'tmdb' END
       FROM work w WHERE w.id = e.work_id),
    e.work_id::text,
    e.collection_id::text
);

ALTER TABLE emprestimo ALTER COLUMN caixa_chave SET NOT NULL;

-- Os quatro índices viram dois: a chave já distingue obra de coleção, porque
-- um uuid nunca colide com um id do TMDB nem com outro uuid.
DROP INDEX emprestimo_uma_copia_work_idx;
DROP INDEX emprestimo_uma_copia_colecao_idx;
DROP INDEX emprestimo_uma_por_pessoa_work_idx;
DROP INDEX emprestimo_uma_por_pessoa_colecao_idx;

-- Uma cópia no mundo, quando a escassez está ligada.
CREATE UNIQUE INDEX emprestimo_uma_copia_idx
    ON emprestimo (caixa_chave)
 WHERE devolvido_em IS NULL AND exclusivo;

-- Uma cópia por pessoa, sempre — inclusive com a escassez desligada.
CREATE UNIQUE INDEX emprestimo_uma_por_pessoa_idx
    ON emprestimo (user_id, caixa_chave)
 WHERE devolvido_em IS NULL;
