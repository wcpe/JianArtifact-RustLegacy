package com.jianartifact.acceptance;

import java.nio.file.Files;
import java.nio.file.Path;
import org.apache.maven.plugin.AbstractMojo;
import org.apache.maven.plugin.MojoExecutionException;

/** 按 Maven 坐标解析构件并写入隔离本地仓库。 */
public final class GetMojo extends AbstractMojo {
    @Override
    public void execute() throws MojoExecutionException {
        try {
            String repository = required("remoteRepositories");
            String[] coordinate = required("artifact").split(":", -1);
            if (coordinate.length != 4) throw new IllegalArgumentException("artifact 必须为 groupId:artifactId:extension:version");
            String relative = coordinate[0].replace('.', '/') + "/" + coordinate[1] + "/" + coordinate[3]
                    + "/" + coordinate[1] + "-" + coordinate[3] + "." + coordinate[2];
            byte[] body = HttpSupport.get(repository + "/" + relative, optionalToken());
            Path target = Path.of(required("maven.repo.local"), relative);
            Files.createDirectories(target.getParent());
            Files.write(target, body);
        } catch (Exception error) {
            throw new MojoExecutionException("Maven dependency:get 验收失败", error);
        }
    }

    private static Path optionalToken() {
        String value = System.getProperty("acceptance.tokenFile");
        return value == null || value.isBlank() ? null : Path.of(value);
    }

    private static String required(String name) {
        String value = System.getProperty(name);
        if (value == null || value.isBlank()) throw new IllegalArgumentException("缺少参数：" + name);
        return value;
    }
}
