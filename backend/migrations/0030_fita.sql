-- R30 — a fita, e quem a deixou zoada.
--
-- ## O achado da R19 estava certo pela metade
--
-- A 0021 escreveu, com orgulho justificado:
--
-- > *"o estado da fita já está no banco. `playback_state` guarda, por usuário e
-- > por obra, onde a pessoa parou. Quem assistiu até o minuto 47 e devolveu
-- > deixou a fita no minuto 47 — isso é literalmente verdade, não simulação."*
--
-- A observação é boa e a conclusão é errada, e o erro só aparece quando se
-- tenta fazer o que foi pedido. **Uma fita é um objeto; `playback_state` é uma
-- memória.** Enquanto a fita for a memória de alguém:
--
--  * rebobinar a fita significa **apagar o "continuar de onde parou" de outra
--    pessoa** — e é exatamente por isso que o §35 recusou fazê-lo, chamando de
--    "ação destrutiva entre usuários". A recusa estava certa; a modelagem é que
--    estava errada;
--  * a fita "anda para trás" sozinha: quem devolveu no minuto 47 e reassistiu
--    amanhã reescreve o passado, e foi preciso congelar `devolvido_como` no
--    empréstimo pra contornar isso;
--  * e **duas pessoas têm fitas diferentes da mesma caixa**, que é a negação do
--    objeto.
--
-- ## A fita é uma coisa, e é dela esta tabela
--
-- Separar as duas dissolve o problema inteiro em vez de contorná-lo:
--
-- | | o que é | de quem é |
-- |---|---|---|
-- | `playback_state` | onde **você** parou | seu, privado, intocável |
-- | `fita` | onde **a fita** está | do acervo, compartilhado |
--
-- Rebobinar passa a mexer no objeto e em ninguém. O "continuar de onde parou"
-- de todo mundo continua intacto, e a única coisa que muda é o que a próxima
-- pessoa encontra ao pôr pra tocar — que é literalmente o que foi pedido:
--
-- > *"saber que estado deixou a fita para o próximo uso"*

CREATE TABLE fita (
    -- Uma fita por obra. **Não por caixa**: uma caixa de série é uma caixa de
    -- fitas, e a fita que ficou no meio é a do episódio que alguém assistiu.
    -- Rebobinar a caixa rebobina as fitas dentro dela, que é o que se faria.
    work_id uuid PRIMARY KEY REFERENCES work(id) ON DELETE CASCADE,

    -- Onde a fita está. Zero é rebobinada.
    posicao_segundos double precision NOT NULL DEFAULT 0
        CHECK (posicao_segundos >= 0),

    -- O comprimento da fita. É propriedade do objeto, não de quem assiste — uma
    -- fita de 118 minutos tem 118 minutos pra quem quer que a ponha no aparelho.
    --
    -- Sem isto, dizer se ela voltou "no meio" ou "até o fim" exigiria buscar a
    -- duração no `playback_state` de alguém, e aí a condição da fita voltaria a
    -- depender da memória de uma pessoa — que é justamente o acoplamento que
    -- esta tabela existe pra cortar.
    duracao_segundos double precision CHECK (duracao_segundos > 0),

    -- Quem deixou assim. **É o nome que faz o atrito existir** — uma fita no
    -- meio sem dono é um defeito do sistema; com dono é uma pessoa que não
    -- rebobinou.
    --
    -- `ON DELETE SET NULL` e não CASCADE: apagar uma conta não deve rebobinar
    -- as fitas que ela deixou pelo caminho. O estado do objeto sobrevive a quem
    -- o produziu — some o nome, fica a fita no minuto 47.
    deixada_por uuid REFERENCES app_user(id) ON DELETE SET NULL,
    deixada_em  timestamptz NOT NULL DEFAULT now()
);

COMMENT ON TABLE fita IS
    'Onde cada fita está, como objeto compartilhado. Não confundir com playback_state, que é a memória de cada pessoa.';

-- "Quem deixou fita zoada por aí" — a pergunta da reputação, feita uma vez por
-- carregamento do balcão.
CREATE INDEX fita_deixada_por_idx ON fita (deixada_por)
    WHERE posicao_segundos > 0;

-- ------------------------------------------------------------------- o log

-- > *"as pessoas saberem quem devolveu zoado e ter que rebobinar"*
--
-- As duas metades da frase estão nesta tabela, e é por isso que ela guarda
-- **dois** nomes. Um rebobinar é sempre um trabalho que alguém teve por causa
-- de alguém: `por` gastou os segundos, `de` deixou assim.
--
-- Sem isto a reputação teria que ser lida do estado atual da `fita`, que só
-- sabe da última pessoa e esquece tudo assim que a fita é rebobinada — ou seja,
-- esqueceria exatamente no instante em que o fato acabou de acontecer.
CREATE TABLE rebobinada (
    id      bigserial PRIMARY KEY,
    work_id uuid NOT NULL REFERENCES work(id) ON DELETE CASCADE,

    -- Quem teve o trabalho.
    por uuid REFERENCES app_user(id) ON DELETE SET NULL,
    -- Quem tinha deixado assim. Nulo quando a fita já estava no começo ou
    -- quando quem deixou não existe mais.
    de  uuid REFERENCES app_user(id) ON DELETE SET NULL,

    -- Quantos segundos voltaram. É o tamanho da bagunça, e é o que a animação
    -- do ponteiro leva pra desfazer.
    segundos double precision NOT NULL CHECK (segundos >= 0),

    quando timestamptz NOT NULL DEFAULT now()
);

-- As duas perguntas da reputação, uma por coluna:
--   "quantas fitas minhas alguém teve que rebobinar"  → (de)
--   "quantas eu rebobinei dos outros"                 → (por)
CREATE INDEX rebobinada_de_idx  ON rebobinada (de);
CREATE INDEX rebobinada_por_idx ON rebobinada (por);

COMMENT ON TABLE rebobinada IS
    'Toda vez que alguém rebobinou a fita de alguém. `por` teve o trabalho, `de` deixou assim.';

-- ------------------------------------------------- de onde vem o estado hoje

-- A fita não nasce vazia: o acervo já sabe onde cada uma está, e jogar isso
-- fora faria a fase começar com todas as fitas rebobinadas — o que é a única
-- coisa que ela não pode dizer, porque não é verdade.
--
-- A posição de partida é o **progresso mais recente de qualquer pessoa** na
-- obra, que é a melhor aproximação do objeto que existe hoje: quem mexeu por
-- último é quem deixou a fita como ela está. Só VHS entra — `year <= 1996`, o
-- mesmo `ULTIMO_ANO_VHS` que a locadora serve à tela —, porque DVD não
-- rebobina, ele lembra onde parou (§35).
--
-- Medido antes de escrever: 18 linhas em `playback_state`, das quais poucas em
-- obras de 1996 ou antes. É semente honesta, não migração de massa.
INSERT INTO fita (work_id, posicao_segundos, duracao_segundos, deixada_por, deixada_em)
SELECT DISTINCT ON (ps.work_id)
       ps.work_id, ps.position_seconds,
       -- `NULLIF` porque a coluna de origem aceita zero e o CHECK daqui não:
       -- duração desconhecida é NULL, não é uma fita de comprimento nenhum.
       NULLIF(ps.duration_seconds, 0), ps.user_id, ps.updated_at
FROM playback_state ps
JOIN work w ON w.id = ps.work_id
WHERE ps.position_seconds > 0
  AND w.year IS NOT NULL AND w.year <= 1996
ORDER BY ps.work_id, ps.updated_at DESC;
