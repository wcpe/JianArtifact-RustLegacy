package com.jianartifact.acceptance;

import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.file.Files;
import java.nio.file.Path;

/** 验收插件共用的最小 HTTP 支持。 */
final class HttpSupport {
    private static final HttpClient CLIENT = HttpClient.newHttpClient();

    private HttpSupport() {}

    static void put(String url, byte[] body, Path tokenFile) throws Exception {
        HttpRequest request = authorized(HttpRequest.newBuilder(URI.create(url)), tokenFile)
                .PUT(HttpRequest.BodyPublishers.ofByteArray(body))
                .build();
        HttpResponse<byte[]> response = CLIENT.send(request, HttpResponse.BodyHandlers.ofByteArray());
        if (response.statusCode() < 200 || response.statusCode() >= 300) {
            throw new IOException("上传失败，HTTP " + response.statusCode() + "：" + url);
        }
    }

    static byte[] get(String url, Path tokenFile) throws Exception {
        HttpRequest request = authorized(HttpRequest.newBuilder(URI.create(url)), tokenFile).GET().build();
        HttpResponse<byte[]> response = CLIENT.send(request, HttpResponse.BodyHandlers.ofByteArray());
        if (response.statusCode() < 200 || response.statusCode() >= 300) {
            throw new IOException("解析失败，HTTP " + response.statusCode() + "：" + url);
        }
        return response.body();
    }

    private static HttpRequest.Builder authorized(HttpRequest.Builder builder, Path tokenFile) throws IOException {
        if (tokenFile != null) {
            String token = Files.readString(tokenFile).trim();
            if (!token.isEmpty()) builder.header("Authorization", "Bearer " + token);
        }
        return builder;
    }
}
