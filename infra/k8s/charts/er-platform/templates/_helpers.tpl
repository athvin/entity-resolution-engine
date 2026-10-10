{{/* The developer name: explicit value, else the release namespace. */}}
{{- define "er-platform.developer" -}}
{{- .Values.developer | default .Release.Namespace -}}
{{- end }}

{{/* §9 Secrets Manager names: er/{env}/{namespace}/{erserver,erweb}. */}}
{{- define "er-platform.secretPath" -}}
er/{{ .Values.environment }}/{{ include "er-platform.developer" . }}
{{- end }}

{{/* Common labels (added to everything). Deliberately WITHOUT
     app.kubernetes.io/instance: that key lives in selectorLabels, and the two
     helpers are composed on pod templates — a duplicate key there breaks
     strict decoding. */}}
{{- define "er-platform.labels" -}}
app.kubernetes.io/part-of: er
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{- end }}

{{/* Selector labels for a component: (dict "root" . "name" "er-api"). */}}
{{- define "er-platform.selectorLabels" -}}
app.kubernetes.io/name: {{ .name }}
app.kubernetes.io/instance: {{ .root.Release.Name }}
{{- end }}

{{/* Image ref for a component: (dict "root" . "image" .Values.images.api).
     Digest beats tag (§14.2 promote-by-digest); component tag beats the
     shared default. */}}
{{- define "er-platform.image" -}}
{{- $reg := .root.Values.images.registry -}}
{{- if .image.digest -}}
{{ $reg }}/{{ .image.repository }}@{{ .image.digest }}
{{- else -}}
{{ $reg }}/{{ .image.repository }}:{{ .image.tag | default .root.Values.images.tag }}
{{- end -}}
{{- end }}

{{/* The lake prefix: explicit value, else dev/{developer} (§7.3). */}}
{{- define "er-platform.lakePrefix" -}}
{{- .Values.erserver.lake.prefix | default (printf "dev/%s" (include "er-platform.developer" .)) -}}
{{- end }}

{{/* The edge hostname, when one is derivable. */}}
{{- define "er-platform.host" -}}
{{- .Values.ingress.host | default (printf "%s.dev.%s" (include "er-platform.developer" .) .Values.ingress.domain) -}}
{{- end }}

{{/* Env shared by er-api and er-dispatcher: everything non-secret the two
     erserver processes read (settings.py); secrets arrive via envFrom from
     the ExternalSecret-projected er-erserver-env. */}}
{{- define "er-platform.erserverEnv" -}}
- name: ERSERVER_ENV
  value: {{ .Values.environment | quote }}
- name: ERSERVER_CONFIG_ROOT
  value: {{ printf "%s/configs" .Values.efs.mountPath | quote }}
- name: ERSERVER_DROP_ROOT
  value: {{ printf "%s/drop" .Values.efs.mountPath | quote }}
- name: ERSERVER_LAKE_DATA_PATH_TEMPLATE
  value: {{ printf "s3://%s/%s/{ns}/" .Values.erserver.lake.bucket (include "er-platform.lakePrefix" .) | quote }}
{{- with .Values.erserver.tenantDbPrefix }}
- name: ERSERVER_TENANT_DB_PREFIX
  value: {{ . | quote }}
{{- end }}
{{- with .Values.erserver.email.from }}
- name: ERSERVER_EMAIL_FROM
  value: {{ . | quote }}
{{- end }}
{{- if or .Values.erserver.email.baseUrl .Values.ingress.enabled }}
- name: ERSERVER_EMAIL_BASE_URL
  value: {{ .Values.erserver.email.baseUrl | default (printf "https://%s" (include "er-platform.host" .)) | quote }}
{{- end }}
{{- end }}

{{/* The pod-level security context all er pods share. The namespace enforces
     PSS baseline (er-dev-namespace); this is most of restricted anyway. */}}
{{- define "er-platform.podSecurityContext" -}}
runAsNonRoot: true
seccompProfile:
  type: RuntimeDefault
{{- end }}

{{- define "er-platform.containerSecurityContext" -}}
allowPrivilegeEscalation: false
capabilities:
  drop: ["ALL"]
{{- end }}
