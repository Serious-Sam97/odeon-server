-- R34 — o guia dinâmico: o ensaio e o evento.
--
-- ## Quase nada precisa de tabela
--
-- O tema da semana e o evento em cartaz são **derivados**: `md5(semana || eixo)`
-- sobre o acervo, com a mesma semente semanal da locadora (§36). Duas visitas na
-- mesma semana veem o mesmo tema, todo mundo vê o mesmo tema, e segunda-feira
-- ele vira sozinho — sem tabela, sem job, sem nada pra expirar.
--
-- É o mesmo truque da emissora (§25) e da vitrine, pela terceira vez. Duas
-- coisas, porém, **não** são deriváveis, e são estas duas tabelas.
--
-- ## 1. O ensaio custa uma chamada a um modelo
--
-- O texto é gerado por LLM sobre fatos vindos do banco (`IDEIAS.md` §2.3), e
-- gerar de novo a cada carregamento da tela seria pagar uma chamada por visita
-- pra receber um texto **diferente** — o que faria a capa do guia mudar de
-- redação enquanto a pessoa lê.
--
-- Então ele é cacheado por (semana, tema). Não é estado de produto: é o
-- resultado de uma função cara, guardado. Apagar a tabela inteira não perde
-- nada que não possa ser gerado de novo.
--
-- ## 2. A participação no evento não sobrevive à janela
--
-- Participar é **terminar a obra do evento enquanto ele está no ar**. O "enquanto
-- está no ar" é o que não dá pra recuperar depois: a semana passa, o tema vira, e
-- saber que alguém terminou *Aliens* exigiria recalcular qual era o evento
-- daquela semana e cruzar com o instante exato do término.
--
-- Dá pra fazer — a semente é determinística —, mas seria reconstruir o passado a
-- cada leitura de perfil, e é a mesma razão pela qual `emprestimo.devolvido_como`
-- é **congelado** (§35) em vez de derivado do `playback_state`.

-- ------------------------------------------------------------------ o ensaio

CREATE TABLE ensaio (
    -- A segunda-feira da semana, e o tema. Juntos são a identidade: dois temas
    -- na mesma semana são dois ensaios, e o mesmo tema em duas semanas também —
    -- o texto fala dos filmes que estavam em cartaz.
    semana date NOT NULL,
    tema   text NOT NULL,

    texto text NOT NULL,

    -- Qual modelo escreveu. **É o selo**, e ele existe pela mesma razão que a
    -- curiosidade da Wikipédia leva o crédito `WIKIPÉDIA` (§32): quem lê tem
    -- direito de saber que aquele parágrafo não foi escrito por gente.
    --
    -- `IDEIAS.md` §2.3 é explícito: *"o que for gerado leva marca"*.
    modelo text NOT NULL,

    gerado_em timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (semana, tema)
);

COMMENT ON TABLE ensaio IS
    'Cache do texto gerado por LLM. Descartável: apagar tudo só custa gerar de novo.';

-- ------------------------------------------------------- o evento em cartaz

-- Uma linha por pessoa por semana. **Não uma por obra**: o evento da semana é um
-- só, e participar dele duas vezes não é participar mais.
CREATE TABLE evento_participacao (
    user_id uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    semana  date NOT NULL,

    -- Qual obra fechou a participação. Guardada porque o evento pode ser uma
    -- **saga** — e aí saber qual das oito obras a pessoa terminou é a diferença
    -- entre "participou" e "participou com o quê".
    --
    -- `ON DELETE SET NULL` e não CASCADE: apagar uma obra do acervo não apaga o
    -- fato de alguém ter estado no evento daquela semana.
    work_id uuid REFERENCES work(id) ON DELETE SET NULL,

    em timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (user_id, semana)
);

-- "De quantos eventos essa pessoa participou" — a pergunta das conquistas.
CREATE INDEX evento_participacao_user_idx ON evento_participacao (user_id);

COMMENT ON TABLE evento_participacao IS
    'Congela o que a janela do evento não deixa recuperar depois: quem terminou a obra em cartaz enquanto ela estava em cartaz.';
