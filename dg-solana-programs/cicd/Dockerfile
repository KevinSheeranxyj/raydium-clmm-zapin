# specific
FROM registry.degate.space/degate/devops-environment:0.0.7

ENV APP_HOME=/usr/local/app
WORKDIR $APP_HOME

COPY config/ /etc/app/templates/
COPY entrypoint.sh /usr/local/bin/

# specific
COPY target/release/main $APP_HOME

ENTRYPOINT ["entrypoint.sh"]
