-- O "terminado" era apagado, e aqui ele é reconstruído.
--
-- `playback_state.finished` vinha de `finished = EXCLUDED.finished` no upsert
-- do `POST /api/works/{id}/progress`. Isso faz do campo o estado do INSTANTE, e
-- não a resposta para "eu já terminei isto alguma vez?" — então reabrir no
-- minuto 30 um filme já visto desmarcava o visto.
--
-- Medido neste servidor antes do conserto: 16 linhas em `playback_state`,
-- ZERO com `finished`, e ainda assim *007: Cassino Royale* com
-- `play_count = 1`. Aquele contador só sobe na transição falso→verdadeiro: ele
-- é o fóssil de um `finished` que existiu e foi sobrescrito.
--
-- Corrigido o upsert, sobra o passado. E ele é recuperável **porque o
-- `play_event` nunca é sobrescrito** — a decisão do §8 ("`play_event`, não
-- `watched: bool`") paga de novo aqui: `playback_state` é um cache derivado, e
-- cache derivado se reconstrói da fonte.
--
-- A regra é a do §8f, a mesma que a curadoria usa e que o guia da R18 passou a
-- usar: terminada é ter evento `finish` OU ter passado de 92% da duração. Sem
-- isso, as duas telas continuariam discordando sobre a mesma palavra, só que
-- agora com o campo consertado e o histórico errado.
--
-- Só LIGA `finished`; nunca desliga. Se alguma linha estiver marcada sem que o
-- log comprove, o log é que está incompleto — eventos podem ter sido perdidos,
-- e apagar a marca destruiria informação que não volta.

UPDATE playback_state ps
SET finished = true
FROM (
    SELECT
        user_id,
        work_id,
        max(position_seconds / NULLIF(duration_seconds, 0))  AS razao,
        count(*) FILTER (WHERE event_type = 'finish')        AS finais
    FROM play_event
    GROUP BY user_id, work_id
) h
WHERE h.user_id = ps.user_id
  AND h.work_id = ps.work_id
  AND NOT ps.finished
  AND (h.finais > 0 OR h.razao >= 0.92);
