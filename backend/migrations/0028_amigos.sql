-- R28 — amigos no lugar do círculo.
--
-- ## O que esta migração desfaz, e por quê
--
-- A 0021 inventou o **círculo**: um grupo fechado, com dono, que passou a
-- escopar empréstimo, rotação, nota, feed, convite e o acesso do convidado. Ele
-- nunca foi pedido. A palavra das anotações originais é **amigos**, e amizade é
-- outra coisa: é entre duas pessoas, não um grupo do qual se é membro.
--
-- A troca não é uma tradução. Se fosse, `circulo` viraria `grupo` e os seis
-- pontos de acoplamento continuariam lá. O que a decisão de hoje diz é mais
-- forte:
--
-- > **O estoque da locadora é do servidor.** Há uma loja, não uma loja por
-- > grupo. Quem entra no Odeon entra nela.
--
-- Com isso o empréstimo deixa de precisar de escopo, e "amigos" passa a existir
-- só onde ele significa alguma coisa: no social — o feed e as notas que você lê
-- antes de assistir. Uma tabela morre em vez de ser renomeada.
--
-- ## O momento é este, e ele não volta
--
-- Medido agora, antes de escrever uma linha:
--
-- | | |
-- |---|---|
-- | círculos | 1 — "A casa", semeado pela própria 0021 |
-- | membros | 2, que são os 2 usuários do servidor |
-- | empréstimos | **0** |
-- | avaliações | **0** |
-- | convites | **0** |
--
-- **Não há um único dado escopado por círculo.** Derrubar a coluna hoje custa
-- um `ALTER TABLE`; daqui a um mês custa decidir a qual grupo pertence cada
-- empréstimo já feito. A 0021 escreveu que adotar o círculo "custa uma coluna e
-- evita uma migração dolorosa" — a conta estava certa e o sinal, invertido.
--
-- ## A única coisa que se perde, e ela é preservada
--
-- Ninguém era membro de um círculo por acidente: as duas pessoas da casa se
-- conhecem. Apagar `circulo_membro` sem mais nada faria as duas acordarem
-- estranhas uma para a outra, e o feed de ambas ficaria vazio no dia seguinte.
-- Então a amizade delas é **semeada a partir da associação que existia** — é a
-- mesma relação, dita direito.

-- ------------------------------------------------------------- a amizade

-- **Par canônico, uma linha por amizade.**
--
-- A alternativa óbvia — uma linha por direção, `de` e `para` — permite que "sam
-- é amigo de rudney" e "rudney é amigo de sam" existam ao mesmo tempo e
-- discordem. Aqui isso é inrepresentável: o par é ordenado pelo uuid e a chave
-- primária é o par. Duas pessoas, uma linha, sempre.
--
-- O preço é uma regra que o código precisa respeitar (ordenar antes de gravar),
-- e ela cabe numa função. O ganho é que "somos amigos?" é uma busca de chave
-- primária, e não um `OR` sobre duas colunas.
CREATE TABLE amizade (
    a uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    b uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,

    -- Quem pediu. É o que dá direção a uma linha que é simétrica de propósito:
    -- sem isto, a tela não sabe se mostra "aceitar" ou "aguardando".
    pedido_por uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    pedido_em  timestamptz NOT NULL DEFAULT now(),

    -- **Nulo é pendente.** Um `estado text` com três valores teria um quarto
    -- estado alcançável — "aceita sem data" — e nenhuma consulta o quer. A data
    -- é a resposta e o carimbo ao mesmo tempo.
    aceito_em  timestamptz,

    PRIMARY KEY (a, b),

    -- A ordem é o que impede a linha duplicada. Sem ele, `(sam, rudney)` e
    -- `(rudney, sam)` são duas chaves diferentes e a garantia acima evapora.
    CONSTRAINT amizade_par_canonico CHECK (a < b),

    -- Quem pediu é um dos dois. Um terceiro emitindo pedido em nome alheio é o
    -- tipo de linha que só aparece quando já estragou alguma coisa.
    CONSTRAINT amizade_pediu_um_dos_dois CHECK (pedido_por = a OR pedido_por = b)
);

-- O par cobre buscas por `a`; `b` precisa do seu. "Meus amigos" olha os dois
-- lados, porque a linha não sabe qual dos dois é você.
CREATE INDEX amizade_b_idx ON amizade (b);

-- "Meus pedidos pendentes" — a pergunta que a tela faz a cada carregamento.
CREATE INDEX amizade_pendente_idx ON amizade (pedido_por) WHERE aceito_em IS NULL;

COMMENT ON TABLE amizade IS
    'Amizade entre duas contas, par ordenado pelo uuid. aceito_em nulo = pedido pendente.';

-- **Recusar apaga a linha.** Não há estado "recusada", e a omissão é escolha:
-- guardar a recusa serviria pra impedir um segundo pedido, o que num servidor
-- de duas pessoas resolve um problema que não existe e cria um pior — quem
-- pediu ficaria vendo "pendente" pra sempre sem saber que já levou não.

-- As duas pessoas da casa já eram amigas; a 0021 só chamava isso de outro nome.
INSERT INTO amizade (a, b, pedido_por, aceito_em)
SELECT least(m1.user_id, m2.user_id),
       greatest(m1.user_id, m2.user_id),
       least(m1.user_id, m2.user_id),
       now()
FROM circulo_membro m1
JOIN circulo_membro m2
  ON m2.circulo_id = m1.circulo_id AND m2.user_id > m1.user_id
ON CONFLICT DO NOTHING;

-- ------------------------------------------- as opções que moravam no círculo

-- Prazo e limite estavam em `circulo`, e a 0021 tinha razão no argumento: "um
-- número de regra de negócio escondido em `const` é um número que ninguém
-- encontra". Ele não pode voltar pro código só porque o dono da coluna sumiu.
--
-- Com uma loja só, o dono é o servidor. Esta tabela é a semente do menu de
-- opções da fase 2 (`IDEIAS.md` §3.2) — que vai acrescentar tamanho do estoque
-- e a chave da escassez. Nasce com as duas colunas que já existem e nenhuma
-- inventada.
--
-- `unica boolean PRIMARY KEY CHECK (unica)` é o singleton imposto pelo banco:
-- uma segunda linha é impossível, e não uma convenção que alguém respeita.
CREATE TABLE locadora_opcoes (
    unica boolean PRIMARY KEY DEFAULT true CHECK (unica),

    prazo_dias        int NOT NULL DEFAULT 7 CHECK (prazo_dias BETWEEN 1 AND 90),
    limite_por_pessoa int NOT NULL DEFAULT 3 CHECK (limite_por_pessoa BETWEEN 1 AND 50)
);

-- Herda o que a casa tinha, em vez de recomeçar no padrão: se alguém já tinha
-- mexido no prazo, mexeu de propósito.
INSERT INTO locadora_opcoes (prazo_dias, limite_por_pessoa)
SELECT prazo_dias, limite_por_membro FROM circulo ORDER BY criado_em LIMIT 1;

-- Servidor sem círculo nenhum (não é o caso aqui, mas a migração não pode
-- depender disso) ainda precisa da linha.
INSERT INTO locadora_opcoes (unica) VALUES (true) ON CONFLICT DO NOTHING;

-- ------------------------------------------------ o empréstimo perde o escopo

-- Os índices de escassez são recriados sem o círculo. **Esta é a mudança de
-- regra**, e ela é a decisão inteira em duas linhas de DDL: uma cópia por caixa
-- **no servidor**, não uma por caixa por grupo.
DROP INDEX emprestimo_uma_copia_work_idx;
DROP INDEX emprestimo_uma_copia_colecao_idx;
DROP INDEX emprestimo_em_aberto_idx;

ALTER TABLE emprestimo DROP COLUMN circulo_id;

CREATE UNIQUE INDEX emprestimo_uma_copia_work_idx
    ON emprestimo (work_id)
    WHERE devolvido_em IS NULL AND work_id IS NOT NULL;

CREATE UNIQUE INDEX emprestimo_uma_copia_colecao_idx
    ON emprestimo (collection_id)
    WHERE devolvido_em IS NULL AND collection_id IS NOT NULL;

-- "O que está fora agora" não ganha índice próprio: sem o `circulo_id` pra
-- prefixar, a pergunta é a varredura do índice parcial de `vence_em`, que já
-- existe (`emprestimo_vencendo_idx`) e já está ordenado pelo que a prateleira
-- quer. Um índice a mais aqui seria a mesma linha lida duas vezes.

-- ------------------------------------------ o convite passa a ser do servidor

-- A 0025 fez o convite apontar pra um círculo porque era o único jeito de dizer
-- "onde" alguém entrava. Sem grupo, a resposta é o servidor — que é o que a
-- palavra "convite" já significava fora do schema.
--
-- O papel `guest` não muda: continua sendo quem só assiste o que pegou
-- emprestado. O que muda é que a autorização deixa de perguntar de qual grupo
-- ele é membro e pergunta só o que sempre importou — se o empréstimo está em
-- aberto.
DROP INDEX convite_circulo_idx;
ALTER TABLE convite DROP COLUMN circulo_id;

CREATE INDEX convite_recentes_idx ON convite (criado_em DESC);

-- ---------------------------------------------------------- o círculo, enfim

DROP TABLE circulo_membro;
DROP TABLE circulo;
