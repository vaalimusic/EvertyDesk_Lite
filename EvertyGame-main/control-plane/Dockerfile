FROM mcr.microsoft.com/dotnet/sdk:8.0 AS build
WORKDIR /src
COPY control-plane/Everty.ControlPlane.csproj control-plane/
RUN dotnet restore control-plane/Everty.ControlPlane.csproj
COPY control-plane/ control-plane/
RUN dotnet publish control-plane/Everty.ControlPlane.csproj -c Release -o /app/publish --no-restore

FROM mcr.microsoft.com/dotnet/aspnet:8.0
WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/*
ENV ASPNETCORE_URLS=http://+:5180
ENV EVERTY_CONTROL_PLANE_STATE_PATH=/data/state.json
ENV EVERTY_CONTROL_PLANE_ACCESS_TOKEN_HOURS=12
ENV EVERTY_CONTROL_PLANE_REFRESH_TOKEN_DAYS=30
ENV EVERTY_CONTROL_PLANE_MAX_REQUEST_BODY_BYTES=1048576
ENV EVERTY_CONTROL_PLANE_DEMO_AUTH_ENABLED=true
ENV EVERTY_CONTROL_PLANE_OPERATOR_KEY=
ENV EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER=manual
ENV EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_ENDPOINT=
ENV EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_API_KEY=
EXPOSE 5180
VOLUME ["/data"]
COPY --from=build /app/publish .
ENTRYPOINT ["dotnet", "Everty.ControlPlane.dll"]
