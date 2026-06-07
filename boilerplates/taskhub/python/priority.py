"""
Sugiere una prioridad 1-5 para una task de TaskHub.

Estrategia:
1. Si OPENAI_API_KEY está set, usa GPT-4o-mini para sugerir
   (real LLM call).
2. Si no, cae a una heurística por keywords (rule-based).

Fitz invoca `suggest_priority(title, description)` y recibe un
Int. Cualquier excepción Python no capturada se vuelve
`Err(Str("ClassName: msg"))` automáticamente en Fitz (Fase 8.3).
"""

import os


def suggest_priority(title: str, description: str) -> int:
    """Punto de entrada llamado desde Fitz."""
    api_key = os.environ.get("OPENAI_API_KEY")

    if api_key:
        try:
            return _llm_priority(title, description, api_key)
        except Exception:
            # Si el LLM falla (network, rate limit, parse error),
            # caemos silenciosamente a la heurística.
            pass

    return _heuristic_priority(title)


def _llm_priority(title: str, description: str, api_key: str) -> int:
    """Llama a OpenAI GPT-4o-mini para sugerir prioridad."""
    import openai  # import lazy — solo si tenemos la key

    client = openai.OpenAI(api_key=api_key)
    prompt = (
        f"Task title: {title}\n"
        f"Description: {description}\n\n"
        "Reply ONLY with a single digit 1-5 indicating priority "
        "(1=lowest, 5=highest)."
    )
    resp = client.chat.completions.create(
        model="gpt-4o-mini",
        messages=[{"role": "user", "content": prompt}],
        max_tokens=5,
        temperature=0,
    )
    text = resp.choices[0].message.content.strip()
    # Clamp 1-5 para que un LLM mal comportado no rompa el shape.
    return max(1, min(5, int(text)))


def _heuristic_priority(title: str) -> int:
    """Fallback rule-based por keywords del title."""
    lower = title.lower()

    # Critical / urgent → 5
    for kw in ("urgent", "asap", "critical", "blocker", "p0"):
        if kw in lower:
            return 5

    # Bugs y fixes → 4
    for kw in ("bug", "fix", "error", "crash", "broken"):
        if kw in lower:
            return 4

    # Refactor / cleanup / tests → 2
    for kw in ("refactor", "cleanup", "test", "docs", "comment"):
        if kw in lower:
            return 2

    # Default → 3 (medium)
    return 3
