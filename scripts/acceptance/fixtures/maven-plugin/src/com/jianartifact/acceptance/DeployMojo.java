package com.jianartifact.acceptance;

import java.nio.file.Files;
import java.nio.file.Path;
import org.apache.maven.plugin.AbstractMojo;
import org.apache.maven.plugin.MojoExecutionException;

/** 按 Maven2 布局部署 release 或 timestamped snapshot。 */
public final class DeployMojo extends AbstractMojo {
    @Override
    public void execute() throws MojoExecutionException {
        try {
            String repository = required("url");
            String groupId = required("groupId");
            String artifactId = required("artifactId");
            String version = required("version");
            Path jar = Path.of(required("file"));
            Path pom = Path.of(required("pomFile"));
            Path tokenFile = Path.of(required("acceptance.tokenFile"));
            String fileVersion = version.endsWith("-SNAPSHOT")
                    ? version.substring(0, version.length() - 9) + "-20250101.010203-1"
                    : version;
            String base = repository + "/" + groupId.replace('.', '/') + "/" + artifactId + "/" + version;
            String name = artifactId + "-" + fileVersion;
            HttpSupport.put(base + "/" + name + ".pom", Files.readAllBytes(pom), tokenFile);
            HttpSupport.put(base + "/" + name + ".jar", Files.readAllBytes(jar), tokenFile);
        } catch (Exception error) {
            throw new MojoExecutionException("Maven deploy 验收失败", error);
        }
    }

    private static String required(String name) {
        String value = System.getProperty(name);
        if (value == null || value.isBlank()) throw new IllegalArgumentException("缺少参数：" + name);
        return value;
    }
}
