{{/* The developer name doubles as the namespace name. Required: a typo here
     creates a whole environment, so there is no default. */}}
{{- define "er-dev-namespace.name" -}}
{{- required "set developer=<name> (make dev-up DEV=<name>)" .Values.developer -}}
{{- end }}

{{- define "er-dev-namespace.labels" -}}
app.kubernetes.io/part-of: er
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/instance: {{ .Release.Name }}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{- end }}
