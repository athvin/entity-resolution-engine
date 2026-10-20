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
# The Rust control plane reads its own env names; map the deployed (Python-era)
# secret keys onto them. Per-org lake/config is carried in each org's row
# (POST /v1/orgs with ER_LAKE_* / ER_CONFIG_PATH), not the ERSERVER_* templates.
- name: ER_PG_DSN
  valueFrom:
    secretKeyRef:
      name: er-erserver-env
      key: ERSERVER_DSN
- name: ER_OPERATOR_TOKEN
  valueFrom:
    secretKeyRef:
      name: er-erserver-env
      key: ERSERVER_OPERATOR_TOKEN
{{/* Email (§7.5). Transport is the SES API, signed by the pod's EKS Pod
     Identity credentials, so there is no credential here to mount — only the
     backend selector, the verified From address and the configuration set.
     An unset backend leaves POST /v1/email answering 503, which the BFF's
     relay already treats as "not configured" and falls back from.

     The Python-era ERSERVER_EMAIL_FROM / ERSERVER_EMAIL_BASE_URL were replaced
     rather than kept beside these: nothing read either one, and BASE_URL
     implied the server builds the links, which it does not — every link
     arrives fully formed in the request's `params`. */}}
{{- with .Values.erserver.email.backend }}
- name: ER_MAIL_BACKEND
  value: {{ . | quote }}
{{- end }}
{{- with .Values.erserver.email.from }}
- name: ER_MAIL_FROM
  value: {{ . | quote }}
{{- end }}
{{- with .Values.erserver.email.configurationSet }}
- name: ER_MAIL_CONFIGURATION_SET
  value: {{ . | quote }}
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
