# M7.C2 — numpy + pandas reales: data analysis

Ejemplo runnable del cap [M7.C2 del curso](../../../../docs/curso/m7-python-interop/c2-numpy-pandas-data-analysis.md).

## Setup

```bash
# 1. Activar venv (creado en C1) e instalar pandas + numpy.
$ source venv/bin/activate
(venv) $ pip install pandas numpy

# 2. Generar el CSV semilla (una sola vez).
(venv) $ python3 generate_data.py
escribí 1008 filas a clima.csv
```

## Run

```bash
(venv) $ fitz-python run app.fitz
{"timestamp":"...","level":"INFO","msg":"server listo"}
```

## Probar

```bash
$ curl -s localhost:3000/stats | head -c 200
[{"mes":1,"ciudad":"Bariloche","temp_promedio":9.8,"temp_desvio":5.13,...

$ curl localhost:3000/percentiles/Bariloche
{"ciudad":"Bariloche","p25":7.1,"p50":12.4,"p75":18.6}

$ curl -i localhost:3000/percentiles/Marte | head -1
HTTP/1.1 500 Internal Server Error
```

## Qué cubre

- pandas + numpy en el venv del proyecto.
- Helper Python que retorna `list[dict]` para marshaling automático
  a `List<Map<Str, Any>>` Fitz.
- Coerción `Map<Str, Any>` → `type` nominal Fitz (`StatRow`,
  `Percentiles`) en bindings con anotación destino (Fase 8.4).
- Excepciones Python (`ValueError`) → `Result::Err` automático
  (Fase 8.3) → 500 HTTP con `{"error":"ValueError: ..."}`.
- Async + `?` para propagación de errores.

## Archivos

- `app.fitz` — handlers HTTP Fitz.
- `weather.py` — helpers Python (pandas/numpy adentro).
- `generate_data.py` — script one-shot que crea `clima.csv`.
- `clima.csv` — generado con `python3 generate_data.py` (no
  committeado; se regenera local).
