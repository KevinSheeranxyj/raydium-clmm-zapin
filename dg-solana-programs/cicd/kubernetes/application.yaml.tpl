apiVersion: apps/v1
kind: Deployment
metadata:
  labels:
    app: {{ SERVICE_NAME }}
  name: {{ SERVICE_NAME }}
  namespace: {{ NAMESPACE }}
spec:
  progressDeadlineSeconds: 600
  replicas: {{ REPLICAS | default(1) }}
  revisionHistoryLimit: 1024
  selector:
    matchLabels:
      app: {{ SERVICE_NAME }}
  strategy:
    type: RollingUpdate
  template:
    metadata:
      annotations:
        prometheus.io/path: /metrics
        prometheus.io/port: "9111"
        prometheus.io/scrape: "true"
      labels:
        app: {{ SERVICE_NAME }}
    spec:
      affinity:
        nodeAffinity:
          requiredDuringSchedulingIgnoredDuringExecution:
            nodeSelectorTerms:
              - matchExpressions:
                  - key: worker
                    operator: In
                    values:
                      - "true"
          preferredDuringSchedulingIgnoredDuringExecution:
            - preference:
                matchExpressions:
                  - key: {{ SERVICE_NAME }}
                    operator: In
                    values:
                      - "true"
              weight: 50
      containers:
        - image: {{ IMAGE }}
          imagePullPolicy: IfNotPresent
          name: {{ SERVICE_NAME }}
          env:
            - name: APP_NAME
              value: {{ SERVICE_NAME }}
          resources:
            limits:
              cpu: "2"
              memory: 4Gi
            requests:
              cpu: 10m
              memory: 16Mi
          securityContext:
            capabilities: {}
          terminationMessagePath: /dev/termination-log
          terminationMessagePolicy: File
          volumeMounts:
            - mountPath: /host-logs
              name: host-logs
            - mountPath: /log
              name: app-log-volume
            - mountPath: /etc/env/app-config
              name: app-config
              readOnly: true
              subPath: app-config
            - mountPath: /etc/env/app-secret
              name: app-secret
              readOnly: true
              subPath: app-secret
        - image: registry.degate.space/base/fluentd:v1.15-9
          imagePullPolicy: IfNotPresent
          name: fluentd-sidecar
          command:
            - sh
            - -c
            - /usr/local/bin/ruby /usr/local/bundle/bin/fluentd --config /fluentd/etc/fluent.conf
              --plugin /fluentd/plugins
          env:
            - name: APP_NAME
              value: {{ SERVICE_NAME }}
            - name: POD_NAME
              valueFrom:
                fieldRef:
                  apiVersion: v1
                  fieldPath: metadata.name
            - name: NAMESPACE
              valueFrom:
                fieldRef:
                  apiVersion: v1
                  fieldPath: metadata.namespace
            - name: NODE_NAME
              valueFrom:
                fieldRef:
                  apiVersion: v1
                  fieldPath: spec.nodeName
          resources:
            limits:
              cpu: 500m
              memory: 1Gi
            requests:
              cpu: 10m
              memory: 16Mi
          terminationMessagePath: /dev/termination-log
          terminationMessagePolicy: File
          volumeMounts:
            - mountPath: /log
              name: app-log-volume
            - mountPath: /host-logs
              name: host-logs
            - mountPath: /fluentd/etc/config
              name: fluentd-sidecar
      dnsPolicy: ClusterFirst
      imagePullSecrets:
        - name: private-registry
      initContainers:
        - command:
            - sh
            - -c
            - mkdir -p /host-logs/$(POD_NAME) && ln -s /host-logs/$(POD_NAME) /log/app
          env:
            - name: POD_NAME
              valueFrom:
                fieldRef:
                  apiVersion: v1
                  fieldPath: metadata.name
          image: busybox
          imagePullPolicy: Always
          name: volume-setup
          resources: {}
          terminationMessagePath: /dev/termination-log
          terminationMessagePolicy: File
          volumeMounts:
            - mountPath: /host-logs
              name: host-logs
            - mountPath: /log
              name: app-log-volume
      restartPolicy: Always
      schedulerName: default-scheduler
      securityContext: {}
      terminationGracePeriodSeconds: 30
      volumes:
        - name: host-logs
          hostPath:
            path: /data/log
            type: DirectoryOrCreate
        - name: app-log-volume
          emptyDir: {}
        - name: fluentd-sidecar
          configMap:
            defaultMode: 420
            name: fluentd-sidecar
        - name: app-config
          configMap:
            defaultMode: 256
            name: app-config
            optional: false
        - name: app-secret
          secret:
            defaultMode: 256
            optional: false
            secretName: app-secret



---
apiVersion: v1
kind: Service
metadata:
  labels:
    app: {{ SERVICE_NAME }}
  name: {{ SERVICE_NAME }}
  namespace: {{ NAMESPACE }}
spec:
  clusterIP: {{ SERVICE_CLUSTER_IP }}
  clusterIPs:
    - {{ SERVICE_CLUSTER_IP }}
  ports:
    - name: port80
      port: 80
      protocol: TCP
      targetPort: 80
    - name: port3000
      port: 3000
      protocol: TCP
      targetPort: 3000
    - name: port3001
      port: 3001
      protocol: TCP
      targetPort: 3001
    - name: port8000
      port: 8000
      protocol: TCP
      targetPort: 8000
    - name: port8080
      port: 8080
      protocol: TCP
      targetPort: 8080
    - name: port9090
      port: 9090
      protocol: TCP
      targetPort: 9090
    - name: port9111
      port: 9111
      protocol: TCP
      targetPort: 9111
  selector:
    app: {{ SERVICE_NAME }}
  sessionAffinity: None
  type: ClusterIP
