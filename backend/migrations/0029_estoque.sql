-- R29 — o estoque, e a chave que desliga a escassez.
--
-- ## O número que estava errado
--
-- A R20 (§36) cortou a vitrine em `CAIXAS_POR_ESTANTE = 16`, o que dá **166
-- caixas** expostas de 600. A escala pedida é outra, e está no `IDEIAS.md` §3.2:
--
-- > A locadora tem **~40 caixas na loja inteira** por semana — não 40 por
-- > estante. O que não está no estoque não existe até o estoque virar.
--
-- 166 não é uma loja pequena, é uma loja. Quarenta é o que cabe numa mesa.
--
-- ## Por que `estoque` mora aqui e não num `const`
--
-- Pelo mesmo argumento que a 0021 escreveu sobre o prazo e que a 0028 honrou ao
-- criar esta tabela: um número de regra de negócio escondido no binário é um
-- número que ninguém encontra. E este em especial é **decidido como opção** —
-- tamanho do estoque, prazo, quantas por pessoa e escassez ligada ou não são
-- itens do menu do servidor.

ALTER TABLE locadora_opcoes
    -- Quantas caixas ficam expostas na loja inteira por semana.
    --
    -- O teto é 1000 e não 600 de propósito: 600 é quantas caixas com pôster
    -- este acervo tem **hoje**, e uma constraint que codifica o tamanho do
    -- acervo vira mentira no dia em que ele cresce. Estoque maior que o acervo
    -- não quebra nada — a loja simplesmente mostra tudo.
    ADD COLUMN estoque int NOT NULL DEFAULT 40 CHECK (estoque BETWEEN 1 AND 1000),

    -- A escassez, ligada ou não.
    --
    -- **Desligada, ela desliga só o bloqueio.** A loja continua com as caixas da
    -- semana — a curadoria por restrição sobrevive, e é o terceiro pilar (§1) —,
    -- mas duas pessoas podem pegar a mesma caixa e o prazo vira lembrete em vez
    -- de tranca. São duas coisas separadas de propósito, e é por isso que são
    -- duas colunas: quem achar a vitrine curta boa e o atrito ruim pode ter as
    -- duas coisas.
    ADD COLUMN escassez boolean NOT NULL DEFAULT true;

COMMENT ON COLUMN locadora_opcoes.estoque IS
    'Caixas expostas na loja inteira por semana. ~40 é a escala pedida (IDEIAS.md §3.2).';
COMMENT ON COLUMN locadora_opcoes.escassez IS
    'Ligada: uma cópia por caixa, e quem recusa o segundo aluguel é o índice único. Desligada: ninguém barra ninguém.';

-- ------------------------------------- o empréstimo passa a saber seu regime

-- ## O problema que esta coluna resolve, e por que ela não é redundante
--
-- Quem recusa o segundo aluguel hoje é um **índice único parcial**, e o §35 fez
-- questão disso: *"quem recusa é o banco, não uma checagem que alguém pode
-- esquecer de escrever no segundo caminho de código"*. É o argumento do §5.
--
-- Uma chave que liga e desliga a exclusividade ameaça exatamente isso. O jeito
-- óbvio de implementá-la é ler `escassez` no handler e pular a inserção
-- conflitante — e aí a regra sai do banco e vira um `if`, com a corrida entre a
-- leitura e o INSERT de volta, que é o defeito que o índice existe pra matar.
--
-- O predicado de um índice parcial não pode consultar outra tabela. Mas pode
-- olhar uma coluna da própria linha. Então o empréstimo passa a **carregar o
-- regime sob o qual nasceu**:
--
-- > o índice único vale sobre empréstimos exclusivos; um não-exclusivo não
-- > disputa com ninguém.
--
-- Isso rende três coisas de graça, e a terceira é a que importa:
--
--  * a regra continua sendo do banco, com a mesma força de antes;
--  * desligar a escassez não afrouxa nada retroativamente — a fita que saiu sob
--    o regime exclusivo continua exclusiva até voltar, que é honesto: ela está
--    com alguém;
--  * ligar a escassez de volta não invalida os empréstimos duplicados que
--    existirem, e nenhum estado impossível precisa ser inventado pra isso.
--
-- `DEFAULT true` porque exclusivo é o padrão do produto, e porque uma linha
-- inserida por engano sem o campo erra pro lado que barra — que é o único lado
-- em que errar é recuperável.
ALTER TABLE emprestimo
    ADD COLUMN exclusivo boolean NOT NULL DEFAULT true;

COMMENT ON COLUMN emprestimo.exclusivo IS
    'O regime em que este empréstimo nasceu. true = disputa a única cópia; false = tirado com a escassez desligada.';

DROP INDEX emprestimo_uma_copia_work_idx;
DROP INDEX emprestimo_uma_copia_colecao_idx;

CREATE UNIQUE INDEX emprestimo_uma_copia_work_idx
    ON emprestimo (work_id)
    WHERE devolvido_em IS NULL AND work_id IS NOT NULL AND exclusivo;

CREATE UNIQUE INDEX emprestimo_uma_copia_colecao_idx
    ON emprestimo (collection_id)
    WHERE devolvido_em IS NULL AND collection_id IS NOT NULL AND exclusivo;

-- ------------------------------------ e ninguém pega a mesma caixa duas vezes

-- Com a escassez ligada isto era consequência do índice acima: se só existe uma
-- cópia em aberto, ninguém pode ter duas. **Desligada, deixa de ser** — e sem
-- esta linha a mesma pessoa acumularia empréstimos repetidos da mesma caixa,
-- queimando o próprio limite com três cópias do mesmo filme.
--
-- Não é regra nova: é a regra antiga deixando de ser subproduto de outra. Ela
-- vale nos dois regimes, e é por isso que não olha `exclusivo`.
CREATE UNIQUE INDEX emprestimo_uma_por_pessoa_work_idx
    ON emprestimo (user_id, work_id)
    WHERE devolvido_em IS NULL AND work_id IS NOT NULL;

CREATE UNIQUE INDEX emprestimo_uma_por_pessoa_colecao_idx
    ON emprestimo (user_id, collection_id)
    WHERE devolvido_em IS NULL AND collection_id IS NOT NULL;
